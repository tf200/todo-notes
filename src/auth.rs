//! Sign-in as one state machine (`rune::machine`): every phase is a variant that carries only
//! what is valid in it, and `step` returns effects instead of performing them, so it is pure and
//! fully unit-tested. `main.rs` carries the effects out (requests, credential store).

use rune::machine::{Attempt, Effects, Seq};

use crate::api::{ApiError, Me};
use crate::session::{Session, Tokens};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    SignIn,
    SignUp,
}

pub const MIN_PASSWORD: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum FormError {
    /// The email field is the problem (empty, malformed).
    Email(&'static str),
    /// The password field is the problem (empty, too short).
    Password(&'static str),
    /// The server said no, or couldn't be reached.
    Api(ApiError),
}

impl FormError {
    pub fn message(&self) -> &'static str {
        match self {
            FormError::Email(m) | FormError::Password(m) => m,
            FormError::Api(e) => e.message(),
        }
    }

    pub fn on_email(&self) -> bool {
        matches!(self, FormError::Email(_))
    }

    pub fn on_password(&self) -> bool {
        matches!(self, FormError::Password(_) | FormError::Api(ApiError::InvalidCredentials))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Auth {
    /// Launching with saved tokens: asking the server whether they're still good (refreshing
    /// them on the way if the access token has run out).
    Checking { attempt: Attempt, session: Session },
    /// The form is showing, with the last problem if there was one.
    SignedOut { error: Option<FormError> },
    /// The form was sent; waiting for the server.
    Submitting { attempt: Attempt, mode: Mode },
    SignedIn { session: Session },
}

impl Default for Auth {
    fn default() -> Self {
        Auth::SignedOut { error: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The app started; `saved` is what the credential store holds, if anything.
    Launch { saved: Option<String>, now: u64 },
    Submit { mode: Mode, email: String, password: String },
    /// `/login` or `/register` answered.
    Answered { attempt: Attempt, result: Result<Tokens, ApiError> },
    /// `/api/me` answered for the saved token.
    Checked { attempt: Attempt, result: Result<Me, ApiError> },
    /// The user edited the form: the old error no longer applies.
    Edited,
    SignOut,
    /// A request was refused even after refreshing: the sign-in has ended.
    Unauthorized,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Use these tokens for this sign-in (they're already saved), and check them with `/api/me`.
    CheckToken { attempt: Attempt, tokens: Tokens },
    Send { attempt: Attempt, mode: Mode, email: String, password: String },
    /// A new sign-in: save these tokens and make requests with them.
    Begin(Tokens),
    /// Forget the saved tokens and stop using them.
    ClearToken,
    /// The user signed out: end the sign-in on the server too, once queued saves have landed.
    Revoke,
    /// Signed in: the password field's contents are no longer needed.
    ForgetPassword,
}

pub fn step(seq: &mut Seq, auth: &mut Auth, event: Event) -> Effects<Effect> {
    match (&*auth, event) {
        // Nothing saved: stay on whatever the app started with (the empty form).
        (_, Event::Launch { saved: None, .. }) => Effects::none(),

        (_, Event::Launch { saved, now }) => {
            let tokens = saved.as_deref().and_then(Tokens::decode).filter(|t| t.usable(now));
            match tokens.and_then(|t| Some((t.claims()?, t))) {
                Some((claims, tokens)) => {
                    let attempt = seq.next();
                    *auth = Auth::Checking { attempt, session: Session { email: claims.email } };
                    Effects::one(Effect::CheckToken { attempt, tokens })
                }
                // Dead or unreadable tokens are removed.
                _ => {
                    *auth = Auth::SignedOut { error: None };
                    Effects::one(Effect::ClearToken)
                }
            }
        }

        (Auth::Checking { attempt, session }, Event::Checked { attempt: a, result })
            if *attempt == a =>
        {
            match result {
                Ok(_) => {
                    *auth = Auth::SignedIn { session: session.clone() };
                    Effects::none()
                }
                Err(ApiError::Unauthorized) => {
                    *auth = Auth::SignedOut { error: Some(FormError::Api(ApiError::Unauthorized)) };
                    Effects::one(Effect::ClearToken)
                }
                // Offline or a server hiccup says nothing about the token: keep it for next time.
                Err(e) => {
                    *auth = Auth::SignedOut { error: Some(FormError::Api(e)) };
                    Effects::none()
                }
            }
        }

        (Auth::SignedOut { .. }, Event::Submit { mode, email, password }) => {
            let email = email.trim().to_string();
            if let Some(error) = validate(mode, &email, &password) {
                *auth = Auth::SignedOut { error: Some(error) };
                return Effects::none();
            }
            let attempt = seq.next();
            *auth = Auth::Submitting { attempt, mode };
            Effects::one(Effect::Send { attempt, mode, email, password })
        }

        (Auth::Submitting { attempt, .. }, Event::Answered { attempt: a, result })
            if *attempt == a =>
        {
            match result.map(|tokens| tokens.claims().map(|c| (c, tokens))) {
                Ok(Some((claims, tokens))) => {
                    *auth = Auth::SignedIn { session: Session { email: claims.email } };
                    Effects::two(Effect::Begin(tokens), Effect::ForgetPassword)
                }
                Ok(None) => {
                    *auth = Auth::SignedOut { error: Some(FormError::Api(ApiError::Server(200))) };
                    Effects::none()
                }
                Err(e) => {
                    *auth = Auth::SignedOut { error: Some(FormError::Api(e)) };
                    Effects::none()
                }
            }
        }

        (Auth::SignedOut { error: Some(_) }, Event::Edited) => {
            *auth = Auth::SignedOut { error: None };
            Effects::none()
        }

        (Auth::SignedIn { .. }, Event::SignOut) => {
            *auth = Auth::SignedOut { error: None };
            Effects::two(Effect::Revoke, Effect::ClearToken)
        }

        (Auth::SignedIn { .. }, Event::Unauthorized) => {
            *auth = Auth::SignedOut { error: Some(FormError::Api(ApiError::Unauthorized)) };
            Effects::one(Effect::ClearToken)
        }

        // Stale answers (an attempt the state no longer holds), a second submit while one is in
        // flight, and anything else out of place change nothing.
        _ => Effects::none(),
    }
}

fn validate(mode: Mode, email: &str, password: &str) -> Option<FormError> {
    if email.is_empty() {
        return Some(FormError::Email("Enter your email."));
    }
    let looks_like_email = email
        .split_once('@')
        .is_some_and(|(user, host)| !user.is_empty() && host.contains('.') && !host.ends_with('.'));
    if !looks_like_email {
        return Some(FormError::Email("That doesn't look like an email address."));
    }
    if password.is_empty() {
        return Some(FormError::Password("Enter your password."));
    }
    if mode == Mode::SignUp && password.chars().count() < MIN_PASSWORD {
        return Some(FormError::Password("Use at least 8 characters."));
    }
    None
}

#[cfg(test)]
mod tests;
