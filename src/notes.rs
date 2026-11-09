//! Notes: what the notes screen shows, and the requests that keep the server in step.
//!
//! Like `todos.rs`: edits change the model at once and queue an [`Op`]; new notes carry a
//! temporary id until the server's create returns; a failed request reloads and leaves a
//! notice. Order is pinned notes first, then the most recently edited: an edit moves its note
//! to the top of its group, as the server will order it on the next load.

use crate::api::{ApiError, Note, NotePatch};
use crate::todos::{Effect, Load, Op, Reply, is_temp, reason};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct Notes {
    pub notes: Load<Vec<Note>>,
    /// The note in the editor.
    pub open: Option<String>,
    pub notice: Option<String>,
    next_temp: u64,
}

impl Default for Notes {
    fn default() -> Self {
        Notes { notes: Load::Loading, open: None, notice: None, next_temp: 0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Signed in: start over and load.
    Open,
    /// A blank note at the top, opened.
    New,
    Select(String),
    /// Saves the editor's title and body.
    Edit { id: String, title: String, body: String },
    SetPinned { id: String, pinned: bool },
    Delete(String),
    Reload,
    DismissNotice,
    Reply(Reply),
}

impl Notes {
    pub fn all(&self) -> &[Note] {
        match &self.notes {
            Load::Ready(n) => n,
            _ => &[],
        }
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.all().iter().find(|n| n.id == id)
    }

    pub fn open_note(&self) -> Option<&Note> {
        self.get(self.open.as_deref()?)
    }

    fn list_mut(&mut self) -> Option<&mut Vec<Note>> {
        match &mut self.notes {
            Load::Ready(n) => Some(n),
            _ => None,
        }
    }

    /// Moves the note at `at` to the top of its group (pinned or not).
    fn to_top(&mut self, at: usize) {
        let Some(notes) = self.list_mut() else { return };
        let note = notes.remove(at);
        let top = if note.pinned { 0 } else { notes.iter().filter(|n| n.pinned).count() };
        notes.insert(top, note);
    }

    fn failed(&mut self, what: &str, e: &ApiError, fx: &mut Vec<Effect>) {
        if *e == ApiError::Unauthorized {
            fx.push(Effect::Unauthorized);
            return;
        }
        self.notice = Some(format!("{what} {}", reason(e)));
        fx.push(Effect::Run(Op::LoadNotes));
    }
}

/// The text a note's row shows under its title: the body's first line with words in it,
/// without Markdown's leading marks.
pub fn preview(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches(['#', '-', '*', '>', ' ']).trim())
        .map(|l| l.strip_prefix("[ ]").or_else(|| l.strip_prefix("[x]")).unwrap_or(l).trim())
        .find(|l| !l.is_empty())
        .unwrap_or("No additional text")
        // Inline marks read as noise in a one-line summary.
        .replace(['*', '`'], "")
        .replace("__", "")
}

/// Pinned first, keeping the server's order (most recently edited first) within each group.
fn pinned_first(notes: &mut [Note]) {
    notes.sort_by_key(|n| !n.pinned);
}

pub fn step(n: &mut Notes, event: Event) -> Vec<Effect> {
    let mut fx = Vec::new();
    match event {
        Event::Open => {
            *n = Notes::default();
            fx.push(Effect::Run(Op::LoadNotes));
        }
        Event::Reload => {
            n.notice = None;
            if matches!(n.notes, Load::Failed(_)) {
                n.notes = Load::Loading;
            }
            fx.push(Effect::Run(Op::LoadNotes));
        }
        Event::DismissNotice => n.notice = None,
        Event::Select(id) => {
            if n.get(&id).is_some() {
                n.open = Some(id);
            }
        }
        Event::New => {
            if !matches!(n.notes, Load::Ready(_)) {
                return fx;
            }
            n.next_temp += 1;
            let tmp = format!("tmp-note-{}", n.next_temp);
            let note = Note {
                id: tmp.clone(),
                title: String::new(),
                body: String::new(),
                pinned: false,
                created_at: String::new(),
                updated_at: String::new(),
            };
            if let Some(notes) = n.list_mut() {
                let top = notes.iter().filter(|x| x.pinned).count();
                notes.insert(top, note);
            }
            n.open = Some(tmp.clone());
            fx.push(Effect::Run(Op::CreateNote { tmp, title: String::new(), body: String::new() }));
        }
        Event::Edit { id, title, body } => {
            let Some(notes) = n.list_mut() else { return fx };
            let Some(at) = notes.iter().position(|x| x.id == id) else { return fx };
            let note = &mut notes[at];
            let mut patch = NotePatch::default();
            if title != note.title {
                note.title = title.clone();
                patch.title = Some(title);
            }
            if body != note.body {
                note.body = body.clone();
                patch.body = Some(body);
            }
            if patch == NotePatch::default() {
                return fx;
            }
            n.to_top(at);
            fx.push(Effect::Run(Op::UpdateNote { id, patch }));
        }
        Event::SetPinned { id, pinned } => {
            let Some(notes) = n.list_mut() else { return fx };
            let Some(at) = notes.iter().position(|x| x.id == id) else { return fx };
            if notes[at].pinned == pinned {
                return fx;
            }
            notes[at].pinned = pinned;
            n.to_top(at);
            fx.push(Effect::Run(Op::UpdateNote { id, patch: NotePatch { pinned: Some(pinned), ..Default::default() } }));
        }
        Event::Delete(id) => {
            let Some(notes) = n.list_mut() else { return fx };
            let Some(at) = notes.iter().position(|x| x.id == id) else { return fx };
            notes.remove(at);
            // Open the note that slides into its place.
            let next = notes.get(at).or_else(|| at.checked_sub(1).and_then(|i| notes.get(i))).map(|x| x.id.clone());
            if n.open.as_ref() == Some(&id) {
                n.open = next;
            }
            fx.push(Effect::Run(Op::DeleteNote { id }));
        }
        Event::Reply(reply) => reply_step(n, reply, &mut fx),
    }
    fx
}

fn reply_step(n: &mut Notes, reply: Reply, fx: &mut Vec<Effect>) {
    match reply {
        Reply::Notes(Ok(mut notes)) => {
            // Notes still being created stay at the top.
            let temps: Vec<Note> = n.all().iter().filter(|x| is_temp(&x.id)).cloned().collect();
            notes.splice(0..0, temps);
            pinned_first(&mut notes);
            if n.open.as_ref().is_some_and(|o| !notes.iter().any(|x| &x.id == o)) {
                n.open = None;
            }
            if n.open.is_none() {
                n.open = notes.first().map(|x| x.id.clone());
            }
            n.notes = Load::Ready(notes);
        }
        Reply::Notes(Err(ApiError::Unauthorized)) => fx.push(Effect::Unauthorized),
        Reply::Notes(Err(e)) => {
            if matches!(n.notes, Load::Ready(_)) {
                n.notice = Some(format!("Couldn't refresh your notes. {}", reason(&e)));
            } else {
                n.notes = Load::Failed(e);
            }
        }
        Reply::NoteCreated { tmp, result: Ok(saved) } => {
            // Keep what was typed while the create was on its way: the worker sends it next.
            if let Some(note) = n.list_mut().and_then(|v| v.iter_mut().find(|x| x.id == tmp)) {
                note.id = saved.id.clone();
                note.created_at = saved.created_at;
                note.updated_at = saved.updated_at;
            }
            if n.open.as_ref() == Some(&tmp) {
                n.open = Some(saved.id);
            }
        }
        Reply::NoteCreated { tmp, result: Err(e) } => {
            if let Some(notes) = n.list_mut() {
                notes.retain(|x| x.id != tmp);
            }
            if n.open.as_ref() == Some(&tmp) {
                n.open = n.all().first().map(|x| x.id.clone());
            }
            n.failed("Couldn't create the note.", &e, fx);
        }
        Reply::NoteSaved(Ok(saved)) => {
            // Only the server's clock: newer local text may already be on its way.
            if let Some(note) = n.list_mut().and_then(|v| v.iter_mut().find(|x| x.id == saved.id)) {
                note.updated_at = saved.updated_at;
            }
        }
        Reply::NoteSaved(Err(e)) => n.failed("Couldn't save the note.", &e, fx),
        Reply::NoteDeleted(Ok(())) => {}
        Reply::NoteDeleted(Err(e)) => n.failed("Couldn't delete the note.", &e, fx),
        // Todo replies go to `todos::step`.
        _ => {}
    }
}

/// Flips the GFM task marker whose `[` is at byte `offset` in `body` (from the Markdown
/// view's `on_task`). `None` when the offset doesn't point at a marker any more.
pub fn toggle_task(body: &str, offset: usize, checked: bool) -> Option<String> {
    let marker = body.get(offset..offset + 3)?;
    if !matches!(marker, "[ ]" | "[x]" | "[X]") {
        return None;
    }
    let mut out = String::with_capacity(body.len());
    out.push_str(&body[..offset + 1]);
    out.push(if checked { 'x' } else { ' ' });
    out.push_str(&body[offset + 2..]);
    Some(out)
}
