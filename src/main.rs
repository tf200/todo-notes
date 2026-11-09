//! Todo & Notes: a Rune desktop client for the todo backend.
//!
//! Run: `cargo run`. Headless (no window, no credential store):
//! `cargo run -- --snap out.png [--scene error] [--theme light]`.
//! Server: `TODO_API_URL` (default `http://localhost:8090`).

mod api;
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

use std::cell::Cell;

use rune::machine::{Attempt, Seq};
use rune::{AppSpec, KeyEvent, Latest, PointerPress, TextEdit, Tx, Waker};
use rune_kit::PaneState;

use api::{Api, ApiError, Me};
use auth::{Auth, Effect, Event, Mode};
use backend::Backend;
use session::{Keyring, NoStore, TokenStore};
use notes::Notes;
use todos::Todos;

/// What a context menu is open on.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuTarget {
    List(String),
    Task(String),
    Note(String),
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
    store: Box<dyn TokenStore>,
    /// `/login` and `/register` answers, tagged with the attempt that asked.
    sent: Latest<(Attempt, Result<String, ApiError>)>,
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
    pub show_done: bool,
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
}

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
                Effect::CheckToken { attempt, token } => {
                    let api = self.api.clone();
                    self.checked.request(token, move |t| (attempt, api.me(&t)));
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
                Effect::SaveToken(token) => self.store.save(&token),
                Effect::ClearToken => self.store.clear(),
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

    fn token(&self) -> Option<&str> {
        match &self.auth {
            Auth::SignedIn { session } => Some(&session.token),
            _ => None,
        }
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
                    if let (Some(b), Some(token)) = (&self.backend, self.token()) {
                        b.run(self.todos.epoch, token, op);
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
                if let (todos::Effect::Run(op), Some(b), Some(token)) = (effect, &self.backend, self.token()) {
                    b.run(self.todos.epoch, token, op);
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
                    if let (Some(b), Some(token)) = (&self.backend, self.token()) {
                        b.run(self.todos.epoch, token, op);
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
        self.todo(todos::Event::OpenTask(Some(id)));
        self.details.set_open(true);
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
    let store: Box<dyn TokenStore> = if headless() { Box::new(NoStore) } else { Box::new(Keyring) };
    let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| api::DEFAULT_BASE_URL.to_string());
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
        show_done: true,
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
    }
}

fn setup(s: &mut App, waker: Waker) {
    s.sent.set_waker(waker.clone());
    s.checked.set_waker(waker.clone());
    s.backend = Some(Backend::start(s.api.clone(), waker));
    let saved = s.store.load();
    s.dispatch(Event::Launch { saved, now: session::now_unix() });
}

fn poll(s: &mut App, now: f64, _tx: &mut Tx) -> bool {
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
    // Due labels say "Today": follow the date past midnight.
    let today = dates::today();
    if today != s.today {
        s.today = today;
        changed = true;
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
