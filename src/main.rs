//! Todo & Notes: a Rune desktop client for the todo backend.
//!
//! Run: `cargo run`. Headless (no window, no credential store):
//! `cargo run -- --snap out.png [--scene error] [--theme light]`.
//! Server: `TODO_API_URL` (default `https://todo.loket.site`; the dev stack is `http://localhost:8090`).

// Release builds are a windowed app on Windows: no console window opens beside it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod attachments;
mod attachments_ui;
mod auth;
mod backdrop;
mod backend;
mod dates;
mod notes;
mod notes_ui;
mod session;
mod todos;
mod todos_ui;
mod ui;
mod updates;

use std::cell::Cell;
use std::sync::Arc;

use rune::machine::{Attempt, Seq};
use rune::{AppSpec, KeyEvent, Latest, PointerPress, TextEdit, Tx, Waker};
use rune_kit::PaneState;

use api::{Api, ApiError, Me};
use auth::{Auth, Effect, Event, Mode};
use backend::Backend;
use session::{Creds, Keyring, NoStore, TokenStore, Tokens};
use notes::Notes;
use todos::Todos;

/// What a context menu is open on.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuTarget {
    List(String),
    Task(String),
    Note(String),
    Attachment(String),
}

/// Which half of the app the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Tasks,
    Notes,
}

pub struct App {
    pub auth: Auth,
    seq: Seq,
    pub mode: Mode,
    pub email: TextEdit,
    pub password: TextEdit,
    pub reveal: bool,
    /// Seconds on the window clock, for the login background's animation.
    pub t: f32,
    /// Set by the view when the system asks for reduced motion: the background holds still and
    /// `poll` stops asking for frames. (Only the view can read the preference.)
    pub still: Cell<bool>,
    api: Api,
    store: Arc<dyn TokenStore>,
    /// The current sign-in's credentials, shared with the threads that make requests.
    creds: Option<Arc<Creds>>,
    /// `/login` and `/register` answers, tagged with the attempt that asked.
    sent: Latest<(Attempt, Result<Tokens, ApiError>)>,
    /// `/api/me` answers for the saved token.
    checked: Latest<(Attempt, Result<Me, ApiError>)>,

    // The todo screen.
    pub todos: Todos,
    backend: Option<Backend>,
    pub sidebar: PaneState,
    pub details: PaneState,
    /// The "Add a task" field.
    pub new_task: TextEdit,
    /// The list being renamed in the sidebar (`NEW_LIST` for a list not yet created).
    pub renaming: Option<String>,
    pub rename: TextEdit,
    /// The details pane's fields, for `todos.open_task`.
    pub task_title: TextEdit,
    pub task_notes: TextEdit,
    /// When the details pane's edits save by themselves (window clock seconds): a short pause
    /// after the last keystroke.
    pub save_due: Option<f64>,
    /// Sections of the task list folded away: a step's, or done's.
    pub collapsed: std::collections::HashSet<todos::Place>,
    /// The details pane's status picker is open.
    pub stage_picker: bool,
    /// The workflow editor, while it's open: the steps as they're being edited.
    pub workflow: Option<Vec<DraftStage>>,
    /// The move the undo toast is showing (`todos::Moved::n`), and when it goes away.
    undo_seen: u64,
    undo_until: f64,
    /// The user's local date, for due labels; kept current by `poll`.
    pub today: chrono::NaiveDate,
    /// The details pane's due-date picker, open on this month.
    pub due_picker: Option<chrono::NaiveDate>,
    pub menu: Option<(MenuTarget, PointerPress)>,
    /// A list waiting on the delete confirmation.
    pub confirm_delete: Option<String>,

    // The notes screen.
    pub section: Section,
    pub notes: Notes,
    /// The editor's fields, holding the note `note_loaded`.
    pub note_title: TextEdit,
    pub note_body: TextEdit,
    note_loaded: Option<String>,
    /// The editor shows the rendered Markdown instead of the source.
    pub note_preview: bool,

    /// Files on tasks and notes, and the threads that move them.
    pub files: attachments::Files,
    transfers: Option<attachments::Transfers>,
    /// A file is being dragged over the open task or note.
    pub drop_over: bool,

    /// New versions, found and downloaded in the background.
    pub updates: updates::Updates,
}

/// A step in the workflow editor.
pub struct DraftStage {
    pub id: String,
    pub name: TextEdit,
    pub color: String,
}

/// Seconds the "Moved to …" toast offers its undo.
const UNDO_FOR: f64 = 6.0;

/// Seconds after the last keystroke before an edit saves itself.
pub const AUTOSAVE_PAUSE: f64 = 0.8;

/// `renaming` for the draft row of a list being created.
pub const NEW_LIST: &str = "new-list";

impl App {
    /// Steps the auth machine and carries out what it asks for.
    pub fn dispatch(&mut self, event: Event) {
        let was_in = self.signed_in();
        for effect in auth::step(&mut self.seq, &mut self.auth, event) {
            match effect {
                Effect::CheckToken { attempt, tokens } => {
                    let creds = self.begin(tokens);
                    let api = self.api.clone();
                    // Refreshes on the way when the saved access token has run out.
                    self.checked.request(creds, move |c| (attempt, c.call(|t| api.me(t))));
                }
                Effect::Send { attempt, mode, email, password } => {
                    let api = self.api.clone();
                    self.sent.request((email, password), move |(email, password)| {
                        let result = match mode {
                            Mode::SignIn => api.login(&email, &password),
                            Mode::SignUp => api.register(&email, &password),
                        };
                        (attempt, result)
                    });
                }
                Effect::Begin(tokens) => {
                    self.store.save(&tokens.encode());
                    self.begin(tokens);
                }
                Effect::Revoke => {
                    if let (Some(b), Some(c)) = (&self.backend, &self.creds) {
                        b.logout(c);
                    }
                }
                Effect::ClearToken => {
                    self.store.clear();
                    if let Some(c) = self.creds.take() {
                        c.detach();
                    }
                }
                Effect::ForgetPassword => {
                    self.password.replace_text("");
                    self.reveal = false;
                }
            }
        }
        if self.signed_in() && !was_in {
            self.open_todos();
        }
    }

    fn signed_in(&self) -> bool {
        matches!(self.auth, Auth::SignedIn { .. })
    }

    /// Makes `tokens` the credentials requests go out with from now on.
    fn begin(&mut self, tokens: Tokens) -> Arc<Creds> {
        if let Some(old) = self.creds.take() {
            old.detach();
        }
        let creds = Creds::new(tokens, Arc::new(self.api.clone()), self.store.clone());
        self.creds = Some(creds.clone());
        creds
    }

    /// The credentials to send requests with, while signed in.
    fn creds(&self) -> Option<&Arc<Creds>> {
        self.creds.as_ref().filter(|_| self.signed_in())
    }

    fn open_todos(&mut self) {
        self.renaming = None;
        self.menu = None;
        self.confirm_delete = None;
        self.new_task.replace_text("");
        self.details.set_open(false);
        self.section = Section::Tasks;
        self.note_loaded = None;
        self.note_preview = false;
        self.files = attachments::Files::default();
        self.drop_over = false;
        self.todo(todos::Event::Open);
        self.note(notes::Event::Open);
    }

    /// Steps the notes model, queues its requests, and keeps the editor on the open note.
    pub fn note(&mut self, event: notes::Event) {
        use notes::Event as N;
        // Events that can change which note is open: save the editor's text first, so
        // nothing typed is dropped when the fields reload.
        let moves = matches!(event, N::Select(_) | N::New | N::Delete(_) | N::Reply(todos::Reply::Notes(_)));
        if moves {
            self.save_open_note();
        }
        // The server named a new note: the editor keeps its fields (and the caret) on it.
        if let N::Reply(todos::Reply::NoteCreated { tmp, result: Ok(saved) }) = &event {
            if self.note_loaded.as_ref() == Some(tmp) && self.notes.open.as_ref() == Some(tmp) {
                self.note_loaded = Some(saved.id.clone());
            }
        }
        for effect in notes::step(&mut self.notes, event) {
            match effect {
                todos::Effect::Run(op) => {
                    if let (Some(b), Some(creds)) = (&self.backend, self.creds()) {
                        b.run(self.todos.epoch, creds, op);
                    }
                }
                todos::Effect::Unauthorized => self.dispatch(Event::Unauthorized),
            }
        }
        if self.notes.open != self.note_loaded {
            let (title, body) =
                self.notes.open_note().map(|n| (n.title.clone(), n.body.clone())).unwrap_or_default();
            self.note_title = TextEdit::new(title);
            self.note_body = TextEdit::new(body);
            self.note_loaded = self.notes.open.clone();
        }
    }

    pub fn set_section(&mut self, section: Section) {
        if self.section == section {
            return;
        }
        self.save_open_task();
        self.save_open_note();
        self.menu = None;
        self.renaming = None;
        self.section = section;
    }

    /// Whether the editor's fields differ from the saved note.
    pub fn note_edited(&self) -> bool {
        let Some(note) = self.note_loaded.as_ref().and_then(|id| self.notes.get(id)) else { return false };
        self.note_title.text != note.title || self.note_body.text != note.body
    }

    pub fn save_open_note(&mut self) {
        if !self.note_edited() {
            return;
        }
        if let Some(id) = self.note_loaded.clone() {
            // Straight to the model: `note` would save again before stepping.
            let event =
                notes::Event::Edit { id, title: self.note_title.text.clone(), body: self.note_body.text.clone() };
            for effect in notes::step(&mut self.notes, event) {
                if let (todos::Effect::Run(op), Some(b), Some(creds)) = (effect, &self.backend, self.creds()) {
                    b.run(self.todos.epoch, creds, op);
                }
            }
        }
    }

    /// Leaves the open note, deleting it when nothing was ever written in it.
    fn leave_note(&mut self) {
        self.save_open_note();
        let empty = self.notes.open_note().filter(|n| n.title.trim().is_empty() && n.body.trim().is_empty());
        if let Some(id) = empty.map(|n| n.id.clone()) {
            self.note(notes::Event::Delete(id));
        }
    }

    pub fn open_note(&mut self, id: String) {
        if self.notes.open.as_ref() == Some(&id) {
            return;
        }
        self.leave_note();
        self.note(notes::Event::Select(id));
    }

    pub fn new_note(&mut self, tx: &mut Tx) {
        self.set_section(Section::Notes);
        // An empty note is already open: reuse it.
        if self.notes.open_note().is_some_and(|n| n.title.is_empty() && n.body.is_empty()) && !self.note_edited() {
            tx.focus("note-title", false);
            return;
        }
        self.leave_note();
        self.note(notes::Event::New);
        self.note_preview = false;
        tx.focus("note-title", false);
    }

    /// A task box in the Markdown preview was clicked: flip its marker and save.
    pub fn toggle_note_task(&mut self, offset: usize, checked: bool) {
        if let Some(body) = notes::toggle_task(&self.note_body.text, offset, checked) {
            self.note_body = TextEdit::new(body);
            self.save_open_note();
        }
    }

    /// Steps the todo model and queues the requests it asks for.
    pub fn todo(&mut self, event: todos::Event) {
        for effect in todos::step(&mut self.todos, event) {
            match effect {
                todos::Effect::Run(op) => {
                    if let (Some(b), Some(creds)) = (&self.backend, self.creds()) {
                        b.run(self.todos.epoch, creds, op);
                    }
                }
                todos::Effect::Unauthorized => self.dispatch(Event::Unauthorized),
            }
        }
    }

    pub fn submit(&mut self) {
        self.dispatch(Event::Submit {
            mode: self.mode,
            email: self.email.text.clone(),
            password: self.password.text.clone(),
        });
    }

    pub fn switch_mode(&mut self) {
        self.mode = match self.mode {
            Mode::SignIn => Mode::SignUp,
            Mode::SignUp => Mode::SignIn,
        };
        self.dispatch(Event::Edited);
    }

    pub fn sign_out(&mut self, tx: &mut Tx) {
        self.save_open_task();
        self.save_open_note();
        self.dispatch(Event::SignOut);
        tx.focus("email", false);
    }

    // Todo screen actions.

    pub fn add_task(&mut self) {
        let title = self.new_task.text.clone();
        self.new_task.replace_text("");
        self.todo(todos::Event::AddTask(title));
    }

    pub fn select_list(&mut self, id: String) {
        if self.todos.selected.as_ref() != Some(&id) {
            self.save_open_task();
            self.details.set_open(false);
        }
        self.todo(todos::Event::Select(id));
    }

    /// Opens a task in the details pane, saving the one open before.
    pub fn open_task(&mut self, id: String) {
        self.save_open_task();
        let Some(task) = self.todos.task(&id) else { return };
        self.task_title = TextEdit::new(task.title.clone());
        self.task_notes = TextEdit::new(task.description.clone());
        self.due_picker = None;
        self.todo(todos::Event::OpenTask(Some(id.clone())));
        self.details.set_open(true);
        // Opening a task again refreshes its files (another device may have added some).
        let owner = (api::Parent::Task, id);
        if attachments::attachable(&owner) {
            self.load_files(owner);
        }
    }

    /// Sets or removes a task's due day, closing the picker.
    pub fn set_due(&mut self, id: String, day: Option<chrono::NaiveDate>) {
        self.due_picker = None;
        self.todo(todos::Event::SetDue { id, due: day.map(dates::stamp) });
    }

    /// Opens the due-date picker on the task's month (this month when it has none), or closes it.
    pub fn toggle_due_picker(&mut self) {
        if self.due_picker.take().is_some() {
            return;
        }
        let due = self.todos.open_task.as_deref().and_then(|id| self.todos.task(id)?.due_at.as_deref().and_then(dates::parse));
        self.due_picker = Some(dates::month_of(due.unwrap_or(self.today)));
    }

    pub fn close_task(&mut self) {
        self.save_open_task();
        self.todo(todos::Event::OpenTask(None));
        self.details.set_open(false);
    }

    /// Whether the details pane's fields differ from the saved task.
    pub fn task_edited(&self) -> bool {
        let Some(task) = self.todos.open_task.as_ref().and_then(|id| self.todos.task(id)) else { return false };
        let title = self.task_title.text.trim();
        (!title.is_empty() && title != task.title) || self.task_notes.text != task.description
    }

    /// A details field changed: save once typing pauses.
    pub fn edited_task(&mut self, tx: &mut Tx) {
        let due = tx.clock() + AUTOSAVE_PAUSE;
        self.save_due = Some(due);
        tx.wake_at(due);
    }

    pub fn save_open_task(&mut self) {
        self.save_due = None;
        if !self.task_edited() {
            return;
        }
        if let Some(id) = self.todos.open_task.clone() {
            self.todo(todos::Event::EditTask {
                id,
                title: self.task_title.text.clone(),
                description: self.task_notes.text.clone(),
            });
        }
    }

    pub fn start_rename(&mut self, id: String, tx: &mut Tx) {
        let title = self.todos.lists().iter().find(|l| l.id == id).map(|l| l.title.clone()).unwrap_or_default();
        self.begin_rename(id, title, tx);
    }

    pub fn start_new_list(&mut self, tx: &mut Tx) {
        self.begin_rename(NEW_LIST.to_string(), String::new(), tx);
    }

    fn begin_rename(&mut self, id: String, title: String, _tx: &mut Tx) {
        self.menu = None;
        self.rename = TextEdit::new(title);
        // Select the whole title, so typing replaces it. A focused model takes focus when the
        // row's field mounts.
        self.rename.anchor = 0;
        self.rename.focused = true;
        self.renaming = Some(id);
    }

    // Attachments.

    /// The open task and the open note, as attachment owners.
    fn open_owners(&self) -> Vec<attachments::Owner> {
        let mut owners = Vec::new();
        if let Some(id) = &self.todos.open_task {
            owners.push((api::Parent::Task, id.clone()));
        }
        if let Some(id) = &self.notes.open {
            owners.push((api::Parent::Note, id.clone()));
        }
        owners
    }

    /// Loads the files of the open task and note when they aren't loaded yet.
    fn sync_files(&mut self) {
        for owner in self.open_owners() {
            if attachments::attachable(&owner) && self.files.of(&owner).is_none() {
                self.load_files(owner);
            }
        }
    }

    /// Asks for an item's files; what's shown stays until the answer.
    fn load_files(&mut self, owner: attachments::Owner) {
        let (Some(t), Some(creds)) = (&self.transfers, self.creds().cloned()) else { return };
        t.list(self.todos.epoch, &creds, owner.clone());
        self.files.lists.entry(owner).or_insert(todos::Load::Loading);
    }

    /// Uploads files (dropped or picked) to the task or note.
    pub fn attach(&mut self, owner: attachments::Owner, paths: &[std::path::PathBuf]) {
        if !attachments::attachable(&owner) {
            return;
        }
        for path in paths {
            match attachments::check_file(path) {
                Err((name, why)) => self.files.refuse_upload(owner.clone(), name, 0, why),
                Ok((name, size)) => {
                    let (Some(t), Some(creds)) = (&self.transfers, self.creds().cloned()) else { return };
                    let (key, sent) = self.files.begin_upload(owner.clone(), name, size);
                    t.upload(self.todos.epoch, &creds, owner.clone(), key, path.clone(), sent);
                }
            }
        }
    }

    /// The system's Open dialog, then uploads the picked file.
    pub fn pick_attachment(&mut self, owner: attachments::Owner, tx: &mut Tx) {
        let options = rune::FileDialogOptions { title: "Attach a file".into(), ..Default::default() };
        tx.open_file(options, move |s: &mut App, result, _: &mut Tx| match result {
            Ok(Some(path)) => s.attach(owner, &[path]),
            Ok(None) => {}
            Err(e) => s.files.notice = Some(format!("Couldn't open the file picker: {e}")),
        });
    }

    fn attachment(&self, owner: &attachments::Owner, id: &str) -> Option<api::Attachment> {
        match self.files.of(owner)? {
            todos::Load::Ready(list) => list.iter().find(|a| a.id == id).cloned(),
            _ => None,
        }
    }

    pub fn open_attachment(&mut self, owner: &attachments::Owner, id: &str) {
        let Some(a) = self.attachment(owner, id) else { return };
        if self.files.busy.contains(id) {
            return;
        }
        let (Some(t), Some(creds)) = (&self.transfers, self.creds().cloned()) else { return };
        self.files.busy.insert(a.id.clone());
        t.open(self.todos.epoch, &creds, a);
    }

    /// The system's Save dialog, then writes the file where the user chose.
    pub fn save_attachment(&mut self, owner: &attachments::Owner, id: &str, tx: &mut Tx) {
        let Some(a) = self.attachment(owner, id) else { return };
        let options = rune::FileDialogOptions {
            title: "Save attachment".into(),
            file_name: Some(attachments::safe_name(&a.name)),
            ..Default::default()
        };
        tx.save_file(options, move |s: &mut App, result, _: &mut Tx| match result {
            Ok(Some(dest)) => {
                let (Some(t), Some(creds)) = (&s.transfers, s.creds().cloned()) else { return };
                s.files.busy.insert(a.id.clone());
                t.save(s.todos.epoch, &creds, a, dest);
            }
            Ok(None) => {}
            Err(e) => s.files.notice = Some(format!("Couldn't open the save dialog: {e}")),
        });
    }

    pub fn remove_attachment(&mut self, owner: &attachments::Owner, id: &str) {
        let (Some(t), Some(creds)) = (&self.transfers, self.creds().cloned()) else { return };
        self.files.busy.insert(id.to_string());
        t.remove(self.todos.epoch, &creds, owner.clone(), id.to_string());
    }

    /// Saves what's open, then closes so Velopack can install the downloaded version and start
    /// it again (closing runs `on_close`, which lets queued saves reach the server first).
    pub fn restart_to_update(&mut self, tx: &mut Tx) {
        self.save_open_task();
        self.save_open_note();
        if self.updates.restart_into_update() {
            tx.close_window();
        }
    }

    // The workflow editor.

    pub fn open_workflow(&mut self) {
        self.menu = None;
        let drafts = self
            .todos
            .stages
            .iter()
            .map(|st| DraftStage { id: st.id.clone(), name: TextEdit::new(st.name.clone()), color: st.color.clone() })
            .collect();
        self.workflow = Some(drafts);
    }

    /// A new, empty step at the end, with the next unused colour; its name field takes focus.
    pub fn add_draft(&mut self, tx: &mut Tx) {
        let Some(w) = &mut self.workflow else { return };
        let used: Vec<&str> = w.iter().map(|d| d.color.as_str()).collect();
        let color = todos_ui::STAGE_COLORS
            .iter()
            .find(|c| !used.contains(c))
            .unwrap_or(&todos_ui::STAGE_COLORS[w.len() % todos_ui::STAGE_COLORS.len()])
            .to_string();
        let id = todos::new_stage_id();
        w.push(DraftStage { id: id.clone(), name: TextEdit::new(""), color });
        tx.focus(&format!("stage-name-{id}"), false);
    }

    pub fn move_draft(&mut self, id: &str, by: isize) {
        let Some(w) = &mut self.workflow else { return };
        let Some(at) = w.iter().position(|d| d.id == id) else { return };
        let to = at as isize + by;
        if to >= 0 && (to as usize) < w.len() {
            w.swap(at, to as usize);
        }
    }

    pub fn remove_draft(&mut self, id: &str) {
        if let Some(w) = &mut self.workflow {
            w.retain(|d| d.id != id);
        }
    }

    pub fn cycle_draft_color(&mut self, id: &str) {
        let Some(d) = self.workflow.as_mut().and_then(|w| w.iter_mut().find(|d| d.id == id)) else { return };
        let colors = &todos_ui::STAGE_COLORS;
        let at = colors.iter().position(|c| *c == d.color).map_or(0, |i| (i + 1) % colors.len());
        d.color = colors[at].to_string();
    }

    /// Saves the editor's steps (blank ones are dropped) and closes it.
    pub fn save_workflow(&mut self) {
        let Some(w) = self.workflow.take() else { return };
        let stages = w
            .into_iter()
            .map(|d| api::Stage { id: d.id, name: d.name.text.clone(), color: d.color })
            .collect();
        self.todo(todos::Event::SaveStages(stages));
    }

    pub fn commit_rename(&mut self) {
        let title = self.rename.text.clone();
        match self.renaming.take() {
            Some(id) if id == NEW_LIST => self.todo(todos::Event::CreateList(title)),
            Some(id) => self.todo(todos::Event::RenameList { id, title }),
            None => {}
        }
    }
}

fn headless() -> bool {
    std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--script" | "--tree" | "--video" | "--frames"))
}

fn init() -> App {
    // Headless runs (screenshots, scripted checks) never read or write the real credential store.
    let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| api::DEFAULT_BASE_URL.to_string());
    let store: Arc<dyn TokenStore> = if headless() { Arc::new(NoStore) } else { Arc::new(Keyring::for_server(&base)) };
    let mut email = TextEdit::new("");
    email.focused = true;
    App {
        auth: Auth::default(),
        seq: Seq::default(),
        mode: Mode::SignIn,
        email,
        password: TextEdit::default().secure(),
        reveal: false,
        t: 0.0,
        still: Cell::new(false),
        api: Api::new(base),
        store,
        creds: None,
        sent: Latest::default(),
        checked: Latest::default(),
        todos: Todos::default(),
        backend: None,
        sidebar: PaneState::new(260.0).range(200.0, 400.0),
        details: PaneState::new(320.0).range(260.0, 480.0).collapsed(),
        new_task: TextEdit::new(""),
        renaming: None,
        rename: TextEdit::new(""),
        task_title: TextEdit::new(""),
        task_notes: TextEdit::new(""),
        save_due: None,
        collapsed: std::collections::HashSet::new(),
        stage_picker: false,
        workflow: None,
        undo_seen: 0,
        undo_until: 0.0,
        today: dates::today(),
        due_picker: None,
        menu: None,
        confirm_delete: None,
        section: Section::Tasks,
        notes: Notes::default(),
        note_title: TextEdit::new(""),
        note_body: TextEdit::new(""),
        note_loaded: None,
        note_preview: false,
        files: attachments::Files::default(),
        transfers: None,
        drop_over: false,
        updates: updates::Updates::off(),
    }
}

fn setup(s: &mut App, waker: Waker) {
    ui::load_logo(waker.clone(), headless());
    if !headless() {
        s.updates = updates::Updates::start(waker.clone());
    }
    s.sent.set_waker(waker.clone());
    s.checked.set_waker(waker.clone());
    s.transfers = Some(attachments::Transfers::start(s.api.clone(), waker.clone()));
    s.backend = Some(Backend::start(s.api.clone(), waker));
    let saved = s.store.load();
    s.dispatch(Event::Launch { saved, now: session::now_unix() });
}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    // The background moves while the sign-in screens show; signed in, it rests.
    if !s.signed_in() && !s.still.get() {
        s.t = now as f32;
        changed = true;
    }
    if let Some((attempt, result)) = s.sent.take() {
        s.dispatch(Event::Answered { attempt, result });
        changed = true;
    }
    if let Some((attempt, result)) = s.checked.take() {
        s.dispatch(Event::Checked { attempt, result });
        changed = true;
    }
    if s.updates.poll() {
        changed = true;
    }
    // Due labels say "Today": follow the date past midnight.
    let today = dates::today();
    if today != s.today {
        s.today = today;
        changed = true;
    }
    // The undo toast: shown for a while after each move, then gone.
    match s.todos.moved.as_ref().map(|m| m.n) {
        Some(n) if n != s.undo_seen => {
            s.undo_seen = n;
            s.undo_until = now + UNDO_FOR;
            tx.wake_at(s.undo_until);
        }
        Some(_) if now >= s.undo_until => {
            s.todo(todos::Event::DismissUndo);
            changed = true;
        }
        _ => {}
    }
    if s.save_due.is_some_and(|due| now >= due) {
        s.save_open_task();
        s.save_open_note();
        changed = true;
    }
    while let Some((epoch, reply)) = s.backend.as_ref().and_then(Backend::take) {
        // Replies for an earlier sign-in are dropped.
        if epoch == s.todos.epoch && s.signed_in() {
            if reply.is_note() {
                s.note(notes::Event::Reply(reply));
            } else {
                s.todo(todos::Event::Reply(reply));
            }
        }
        changed = true;
    }
    while let Some((epoch, reply)) = s.transfers.as_ref().and_then(attachments::Transfers::take) {
        if epoch == s.todos.epoch && s.signed_in() && s.files.apply(reply) {
            s.dispatch(Event::Unauthorized);
        }
        changed = true;
    }
    // Uploads under way: redraw their progress (their threads wake the window as they go).
    if s.files.uploads.iter().any(|u| u.failed.is_none()) {
        changed = true;
    }
    if s.signed_in() {
        s.sync_files();
    }
    changed
}

fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.signed_in() {
        return false;
    }
    if ev.is_command('n') {
        match (s.section, ev.mods.shift) {
            (Section::Notes, _) => s.new_note(tx),
            (Section::Tasks, true) => s.start_new_list(tx),
            (Section::Tasks, false) => tx.focus("new-task", false),
        }
        return true;
    }
    // Ctrl+1 and Ctrl+2 switch between tasks and notes.
    if ev.is_command('1') || ev.is_command('2') {
        s.set_section(if ev.is_command('1') { Section::Tasks } else { Section::Notes });
        return true;
    }
    if rune_kit::shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    false
}

fn main() {
    // First, before anything else: Velopack's install, update and uninstall hooks run the
    // exe with special arguments and exit here. It also applies an update downloaded earlier.
    velopack::VelopackApp::build().run();
    // The window's, taskbar's and Dock's icon (Linux takes it from the AppImage's .desktop).
    let _ = rune::set_app_icon(ui::ICON_PNG);
    rune::App::new(AppSpec {
        min_size: Some((420.0, 560.0)),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: ui::SCENES,
        ..AppSpec::new("Todo & Notes", (1040.0, 700.0), init, ui::view)
    })
    .on_close(|s, _| {
        s.save_open_task();
        s.save_open_note();
        // Let queued saves reach the server before the process ends.
        if let Some(b) = &mut s.backend {
            b.finish(std::time::Duration::from_secs(3));
        }
        s.sent.cancel();
        s.checked.cancel();
    })
    .run();
}
