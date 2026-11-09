//! Todo & Notes: a Rune desktop client for the todo backend.
//!
//! Run: `cargo run`. Headless (no window, no credential store):
//! `cargo run -- --snap out.png [--scene error] [--theme light]`.
//! Server: `TODO_API_URL` (default `http://localhost:8090`).

mod api;
mod auth;
mod backdrop;
mod backend;
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
use todos::Todos;

/// What a context menu is open on.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuTarget {
    List(String),
    Task(String),
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
    pub show_done: bool,
    pub menu: Option<(MenuTarget, PointerPress)>,
    /// A list waiting on the delete confirmation.
    pub confirm_delete: Option<String>,
}

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
        self.todo(todos::Event::Open);
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
        self.todo(todos::Event::OpenTask(Some(id)));
        self.details.set_open(true);
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

    pub fn save_open_task(&mut self) {
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
        show_done: true,
        menu: None,
        confirm_delete: None,
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
    while let Some((epoch, reply)) = s.backend.as_ref().and_then(Backend::take) {
        // Replies for an earlier sign-in are dropped.
        if epoch == s.todos.epoch && s.signed_in() {
            s.todo(todos::Event::Reply(reply));
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
        if ev.mods.shift {
            s.start_new_list(tx);
        } else {
            tx.focus("new-task", false);
        }
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
        s.sent.cancel();
        s.checked.cancel();
    })
    .run();
}
