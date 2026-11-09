//! Todo & Notes: a Rune desktop client for the todo backend.
//!
//! Run: `cargo run`. Headless (no window, no credential store):
//! `cargo run -- --snap out.png [--scene error] [--theme light]`.
//! Server: `TODO_API_URL` (default `http://localhost:8090`).

mod api;
mod auth;
mod backdrop;
mod session;
mod ui;

use std::cell::Cell;

use rune::machine::{Attempt, Seq};
use rune::{AppSpec, Latest, TextEdit, Tx, Waker};

use api::{Api, ApiError, Me};
use auth::{Auth, Effect, Event, Mode};
use session::{Keyring, NoStore, TokenStore};

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
}

impl App {
    /// Steps the auth machine and carries out what it asks for.
    pub fn dispatch(&mut self, event: Event) {
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
        self.dispatch(Event::SignOut);
        tx.focus("email", false);
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
    }
}

fn setup(s: &mut App, waker: Waker) {
    s.sent.set_waker(waker.clone());
    s.checked.set_waker(waker);
    let saved = s.store.load();
    s.dispatch(Event::Launch { saved, now: session::now_unix() });
}

fn poll(s: &mut App, now: f64, _tx: &mut Tx) -> bool {
    let mut changed = false;
    // The background moves while the sign-in screens show; signed in, it rests.
    if !matches!(s.auth, Auth::SignedIn { .. }) && !s.still.get() {
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
    changed
}

fn main() {
    rune::App::new(AppSpec {
        min_size: Some((420.0, 560.0)),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        scenes: ui::SCENES,
        ..AppSpec::new("Todo & Notes", (960.0, 680.0), init, ui::view)
    })
    .on_close(|s, _| {
        s.sent.cancel();
        s.checked.cancel();
    })
    .run();
}
