//! Attachments: files on tasks and notes.
//!
//! The bytes never pass through the API server. It hands out short-lived signed links, and the
//! file goes straight to (or comes straight from) the file store: an upload is "start" on the
//! API, a PUT to the link, then "complete" on the API. Opening a file downloads it into a cache
//! folder first, then hands it to the system's default app.
//!
//! Transfers run on threads of their own, not on the todo backend's ordered queue, so a large
//! upload never holds up saving an edit. [`Files`] is the model the UI draws; [`Transfers`]
//! carries out the work and hands back [`Reply`]s for it.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use rune::Waker;

use crate::api::{ATTACHMENT_MAX, Api, ApiError, Attachment, Parent};
use crate::session::Creds;
use crate::todos::{Load, is_temp};

/// What a file hangs off: a task or a note, by id.
pub type Owner = (Parent, String);

/// A file on its way up.
#[derive(Debug)]
pub struct Upload {
    pub key: u64,
    pub owner: Owner,
    pub name: String,
    pub size: u64,
    /// Bytes sent so far, counted by the upload's thread.
    pub sent: Arc<AtomicU64>,
    /// Why it stopped, if it did. The row stays until dismissed.
    pub failed: Option<String>,
}

impl Upload {
    /// 0..=1, how much is sent.
    pub fn progress(&self) -> f32 {
        if self.size == 0 {
            return 0.0;
        }
        (self.sent.load(Ordering::Relaxed) as f64 / self.size as f64).min(1.0) as f32
    }
}

/// The files the UI knows about.
#[derive(Debug, Default)]
pub struct Files {
    pub lists: HashMap<Owner, Load<Vec<Attachment>>>,
    pub uploads: Vec<Upload>,
    /// Attachments being opened, saved or removed (their rows show a spinner).
    pub busy: HashSet<String>,
    /// The last thing that went wrong opening, saving or removing a file.
    pub notice: Option<String>,
    next_key: u64,
}

/// Why a transfer failed: the API said no, or something on this machine went wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Api(ApiError),
    Local(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::Api(e) => e.message().to_string(),
            Failure::Local(m) => m.clone(),
        }
    }
}

impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Failure {
        Failure::Api(e)
    }
}

/// What a transfer thread hands back.
#[derive(Debug)]
pub enum Reply {
    Listed { owner: Owner, result: Result<Vec<Attachment>, ApiError> },
    Uploaded { key: u64, result: Result<Attachment, Failure> },
    Opened { id: String, result: Result<(), Failure> },
    Saved { id: String, result: Result<(), Failure> },
    Removed { owner: Owner, id: String, result: Result<(), ApiError> },
}

impl Files {
    pub fn of(&self, owner: &Owner) -> Option<&Load<Vec<Attachment>>> {
        self.lists.get(owner)
    }

    pub fn uploads_of<'a>(&'a self, owner: &'a Owner) -> impl Iterator<Item = &'a Upload> + 'a {
        self.uploads.iter().filter(move |u| &u.owner == owner)
    }

    /// Adds an upload row; the counter is what the upload's thread advances.
    pub fn begin_upload(&mut self, owner: Owner, name: String, size: u64) -> (u64, Arc<AtomicU64>) {
        self.next_key += 1;
        let sent = Arc::new(AtomicU64::new(0));
        self.uploads.push(Upload { key: self.next_key, owner, name, size, sent: sent.clone(), failed: None });
        (self.next_key, sent)
    }

    /// An upload that can't start (too big, not a file): a failed row, so the user sees why.
    pub fn refuse_upload(&mut self, owner: Owner, name: String, size: u64, why: String) {
        let (key, _) = self.begin_upload(owner, name, size);
        if let Some(u) = self.uploads.iter_mut().find(|u| u.key == key) {
            u.failed = Some(why);
        }
    }

    pub fn dismiss_upload(&mut self, key: u64) {
        self.uploads.retain(|u| u.key != key);
    }

    /// Takes in a reply. True when it says the sign-in has ended.
    pub fn apply(&mut self, reply: Reply) -> bool {
        let unauthorized = |e: &ApiError| *e == ApiError::Unauthorized;
        match reply {
            Reply::Listed { owner, result } => match result {
                Ok(list) => {
                    self.lists.insert(owner, Load::Ready(list));
                }
                Err(e) => {
                    let gone = unauthorized(&e);
                    // A failed refresh keeps what's already shown.
                    if !matches!(self.lists.get(&owner), Some(Load::Ready(_))) {
                        self.lists.insert(owner, Load::Failed(e));
                    }
                    return gone;
                }
            },
            Reply::Uploaded { key, result } => {
                let Some(i) = self.uploads.iter().position(|u| u.key == key) else { return false };
                match result {
                    Ok(a) => {
                        let up = self.uploads.remove(i);
                        if let Some(Load::Ready(list)) = self.lists.get_mut(&up.owner) {
                            list.retain(|x| x.id != a.id);
                            list.push(a);
                        } else {
                            self.lists.insert(up.owner, Load::Ready(vec![a]));
                        }
                    }
                    Err(e) => {
                        self.uploads[i].failed = Some(e.message());
                        return e == Failure::Api(ApiError::Unauthorized);
                    }
                }
            }
            Reply::Opened { id, result } | Reply::Saved { id, result } => {
                self.busy.remove(&id);
                if let Err(e) = result {
                    self.notice = Some(e.message());
                    return e == Failure::Api(ApiError::Unauthorized);
                }
            }
            Reply::Removed { owner, id, result } => {
                self.busy.remove(&id);
                match result {
                    // Gone already counts as removed.
                    Ok(()) | Err(ApiError::NotFound) => {
                        if let Some(Load::Ready(list)) = self.lists.get_mut(&owner) {
                            list.retain(|x| x.id != id);
                        }
                    }
                    Err(e) => {
                        self.notice = Some(e.message().to_string());
                        return unauthorized(&e);
                    }
                }
            }
        }
        false
    }
}

/// Runs transfers, each on a thread of its own, and collects their replies.
pub struct Transfers {
    api: Api,
    /// For the file store's links: no overall deadline (a big file on a slow line takes time),
    /// but a connection that stalls still gives up.
    store: ureq::Agent,
    send: Sender<(u64, Reply)>,
    replies: Receiver<(u64, Reply)>,
    waker: Waker,
}

impl Transfers {
    pub fn start(api: Api, waker: Waker) -> Transfers {
        let store = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_send_body(Some(Duration::from_secs(30 * 60)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
            .http_status_as_error(false)
            .build()
            .into();
        let (send, replies) = channel();
        Transfers { api, store, send, replies, waker }
    }

    /// The next finished transfer, tagged with the sign-in epoch that asked for it.
    pub fn take(&self) -> Option<(u64, Reply)> {
        self.replies.try_recv().ok()
    }

    fn spawn(&self, epoch: u64, job: impl FnOnce(&Api, &ureq::Agent, &Waker) -> Reply + Send + 'static) {
        let (api, store, send, waker) = (self.api.clone(), self.store.clone(), self.send.clone(), self.waker.clone());
        let _ = std::thread::Builder::new().name("attachment".into()).spawn(move || {
            let reply = job(&api, &store, &waker);
            if send.send((epoch, reply)).is_ok() {
                waker();
            }
        });
    }

    pub fn list(&self, epoch: u64, creds: &Arc<Creds>, owner: Owner) {
        let creds = creds.clone();
        self.spawn(epoch, move |api, _, _| {
            let result = creds.call(|t| api.attachments(t, owner.0, &owner.1));
            Reply::Listed { owner, result }
        });
    }

    pub fn upload(&self, epoch: u64, creds: &Arc<Creds>, owner: Owner, key: u64, path: PathBuf, sent: Arc<AtomicU64>) {
        let creds = creds.clone();
        self.spawn(epoch, move |api, store, waker| {
            let result = upload(api, store, &creds, &owner, &path, sent, waker);
            Reply::Uploaded { key, result }
        });
    }

    /// Downloads the file (unless it's cached already) and opens it in its default app.
    pub fn open(&self, epoch: u64, creds: &Arc<Creds>, a: Attachment) {
        let creds = creds.clone();
        self.spawn(epoch, move |api, store, _| {
            let result = cached(api, store, &creds, &a).and_then(|path| open_local(&path));
            Reply::Opened { id: a.id, result }
        });
    }

    /// Writes the file to `dest`, from the cache when it's there.
    pub fn save(&self, epoch: u64, creds: &Arc<Creds>, a: Attachment, dest: PathBuf) {
        let creds = creds.clone();
        self.spawn(epoch, move |api, store, _| {
            let result = cached(api, store, &creds, &a).and_then(|path| {
                std::fs::copy(&path, &dest).map(|_| ()).map_err(|e| local("Couldn't save the file", &e))
            });
            Reply::Saved { id: a.id, result }
        });
    }

    pub fn remove(&self, epoch: u64, creds: &Arc<Creds>, owner: Owner, id: String) {
        let creds = creds.clone();
        self.spawn(epoch, move |api, _, _| {
            let result = creds.call(|t| api.delete_attachment(t, &id));
            if result.is_ok() {
                let _ = std::fs::remove_dir_all(cache_dir().join(&id));
            }
            Reply::Removed { owner, id, result }
        });
    }
}

/// Whether a file can be attached to this owner now: items not yet saved have no id to attach to.
pub fn attachable(owner: &Owner) -> bool {
    !is_temp(&owner.1)
}

/// Checks a dropped or picked path before uploading it: its name and size, or why not.
pub fn check_file(path: &Path) -> Result<(String, u64), (String, String)> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return Err((name, local("Couldn't read the file", &e).message())),
    };
    if !meta.is_file() {
        return Err((name, "Folders can't be attached. Attach the files inside instead.".into()));
    }
    if meta.len() == 0 {
        return Err((name, "The file is empty.".into()));
    }
    if meta.len() > ATTACHMENT_MAX {
        return Err((name, ApiError::TooLarge.message().into()));
    }
    Ok((name, meta.len()))
}

fn upload(
    api: &Api,
    store: &ureq::Agent,
    creds: &Creds,
    owner: &Owner,
    path: &Path,
    sent: Arc<AtomicU64>,
    waker: &Waker,
) -> Result<Attachment, Failure> {
    let (name, size) = check_file(path).map_err(|(_, why)| Failure::Local(why))?;
    let file = File::open(path).map_err(|e| local("Couldn't read the file", &e))?;
    let mime = mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string();
    let (attachment, link) = creds.call(|t| api.start_attachment(t, owner.0, &owner.1, &name, size, &mime))?;

    let mut req = store.put(&link.url);
    for (k, v) in &link.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    // The link is signed for this exact length: send it, not a chunked body.
    if !link.headers.keys().any(|k| k.eq_ignore_ascii_case("content-length")) {
        req = req.header("Content-Length", size.to_string());
    }
    let body = Counting { inner: file, sent, size, last: 0, waker: waker.clone() };
    let resp = req
        .send(ureq::SendBody::from_owned_reader(body))
        .map_err(|e| Failure::Local(format!("The upload was interrupted ({e}). Check your connection and try again.")))?;
    if !resp.status().is_success() {
        return Err(Failure::Local(format!("The file store refused the upload ({}).", resp.status().as_u16())));
    }
    Ok(creds.call(|t| api.complete_attachment(t, &attachment.id))?)
}

/// Reads the file to upload, counting the bytes and asking for a frame every percent or so.
struct Counting {
    inner: File,
    sent: Arc<AtomicU64>,
    size: u64,
    last: u64,
    waker: Waker,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let total = self.sent.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if total - self.last >= (self.size / 100).max(64 * 1024) || total == self.size {
            self.last = total;
            (self.waker)();
        }
        Ok(n)
    }
}

/// Where downloaded attachments are kept: one folder per attachment, named by its id, holding
/// the file under its own name (so the app that opens it shows that name).
fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("todo-notes").join("attachments")
}

/// The attachment's file in the cache, downloaded first if it isn't there yet.
fn cached(api: &Api, store: &ureq::Agent, creds: &Creds, a: &Attachment) -> Result<PathBuf, Failure> {
    let dir = cache_dir().join(safe_name(&a.id));
    let path = dir.join(safe_name(&a.name));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == a.size) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| local("Couldn't make room for the file", &e))?;
    let url = creds.call(|t| api.attachment_link(t, &a.id))?;
    let unreachable = || Failure::Local("The download was interrupted. Check your connection and try again.".into());
    let mut resp = store.get(&url).call().map_err(|_| unreachable())?;
    if !resp.status().is_success() {
        return Err(Failure::Local(format!("The file store refused the download ({}).", resp.status().as_u16())));
    }
    // Into a side file, renamed once complete: a cut-off download never passes for the file.
    let part = dir.join(".download");
    let mut out = File::create(&part).map_err(|e| local("Couldn't save the file", &e))?;
    let mut body = resp.body_mut().with_config().limit(ATTACHMENT_MAX + 1).reader();
    std::io::copy(&mut body, &mut out).map_err(|_| unreachable())?;
    out.flush().map_err(|e| local("Couldn't save the file", &e))?;
    drop(out);
    std::fs::rename(&part, &path).map_err(|e| local("Couldn't save the file", &e))?;
    Ok(path)
}

/// Opens a downloaded file in its default app. Files that would run as programs aren't run:
/// their folder opens instead, and the user decides.
fn open_local(path: &Path) -> Result<(), Failure> {
    let target = if runs_as_program(path) { path.parent().unwrap_or(path) } else { path };
    open::that_detached(target).map_err(|e| local("Couldn't open the file", &e))
}

/// Extensions the system would run rather than show.
fn runs_as_program(path: &Path) -> bool {
    const PROGRAMS: &[&str] = &[
        "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "scr",
        "pif", "lnk", "url", "hta", "cpl", "jar", "reg", "inf", "sh", "command", "app", "appimage", "desktop",
    ];
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| PROGRAMS.iter().any(|p| p.eq_ignore_ascii_case(e)))
}

/// A file name that's safe on every desktop: no path separators, reserved characters or
/// Windows device names.
pub fn safe_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    out = out.trim_end_matches(['.', ' ']).trim_start().to_string();
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if out.is_empty() || reserved {
        out = format!("_{out}");
    }
    out
}

fn local(what: &str, e: &std::io::Error) -> Failure {
    Failure::Local(format!("{what}: {e}."))
}

/// "1.2 MB", "340 KB", "12 bytes".
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} bytes")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests;
