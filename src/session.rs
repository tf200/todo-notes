//! The signed-in session: the tokens, what they say about the user, where they're kept between
//! launches (Windows Credential Manager, never a plain file), and [`Creds`], which keeps the
//! access token fresh for every request.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::api::ApiError;

const KEYRING_SERVICE: &str = "todo-notes";
const KEYRING_USER: &str = "session-token";

/// Who is signed in, as the screens show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub email: String,
}

/// What the server hands out at sign-in: a short-lived access JWT, sent with every request,
/// and a refresh token that gets the next pair. Old servers (and tokens saved by older builds)
/// have no refresh token; those sessions end when the access token does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
}

/// What the access token says, read without verifying it: the server verifies; the client only
/// needs the email to show and the expiry to refresh ahead of time.
#[derive(Debug, Clone, PartialEq)]
pub struct Claims {
    pub email: String,
    /// Unix seconds, from the token's `exp` claim.
    pub expires_at: u64,
}

impl Claims {
    pub fn read(jwt: &str) -> Option<Claims> {
        let payload = jwt.split('.').nth(1)?;
        let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(Claims {
            email: claims.get("email")?.as_str()?.to_string(),
            expires_at: claims.get("exp")?.as_u64()?,
        })
    }

    /// Expired, or within a minute of it (so a request doesn't race the deadline).
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix + 60 >= self.expires_at
    }
}

impl Tokens {
    /// Reads what the credential store holds: JSON, or a bare JWT from builds before refresh.
    pub fn decode(saved: &str) -> Option<Tokens> {
        let tokens = serde_json::from_str(saved).ok().unwrap_or(Tokens { access: saved.to_string(), refresh: None });
        tokens.claims().map(|_| tokens)
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn claims(&self) -> Option<Claims> {
        Claims::read(&self.access)
    }

    /// Whether these can still get a request through: a live access token, or a way to a new one.
    pub fn usable(&self, now_unix: u64) -> bool {
        self.refresh.is_some() || self.claims().is_some_and(|c| !c.is_expired(now_unix))
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where the tokens live between launches.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str);
    fn clear(&self);
}

/// Windows Credential Manager (Keychain on macOS, Secret Service on Linux), one entry per
/// server: a sign-in on the dev server is never sent to the live one.
pub struct Keyring {
    user: String,
}

impl Keyring {
    pub fn for_server(base: &str) -> Keyring {
        let user = if base == crate::api::DEFAULT_BASE_URL {
            KEYRING_USER.to_string()
        } else {
            format!("{KEYRING_USER} {base}")
        };
        Keyring { user }
    }

    fn entry(&self) -> Option<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &self.user).ok()
    }
}

impl TokenStore for Keyring {
    fn load(&self) -> Option<String> {
        self.entry()?.get_password().ok()
    }

    fn save(&self, token: &str) {
        if let Some(entry) = self.entry() {
            let _ = entry.set_password(token);
        }
    }

    fn clear(&self) {
        if let Some(entry) = self.entry() {
            let _ = entry.delete_credential();
        }
    }
}

/// Keeps nothing: headless runs and tests never touch the real credential store.
pub struct NoStore;

impl TokenStore for NoStore {
    fn load(&self) -> Option<String> {
        None
    }
    fn save(&self, _: &str) {}
    fn clear(&self) {}
}

/// The server's session endpoints, as [`Creds`] needs them (a trait, so tests can fake it).
pub trait Renew: Send + Sync {
    /// `POST /refresh`: the next pair. `Unauthorized` when the sign-in has ended.
    fn refresh(&self, refresh: &str) -> Result<Tokens, ApiError>;
    /// `POST /logout`, best effort.
    fn logout(&self, refresh: &str);
}

/// One sign-in's credentials, shared by every thread that makes requests for it.
///
/// [`Creds::call`] sends a request with the current access token, refreshing first when it is
/// about to expire, and once more (then retrying) if the server answers 401 anyway. A refresh
/// replaces both tokens (each refresh token works once) and saves them, so the next launch
/// starts from the new pair. Only one refresh runs at a time; a thread that waited on another's
/// refresh uses its result instead of refreshing again with a token that's now spent.
pub struct Creds {
    /// `None` once the sign-in has ended: refused by the server, or logged out.
    tokens: Mutex<Option<Tokens>>,
    server: Arc<dyn Renew>,
    store: Arc<dyn TokenStore>,
    /// Cleared at sign-out: requests still queued may finish (and refresh), but nothing is
    /// written back to the credential store the user just signed out of.
    persist: AtomicBool,
    clock: fn() -> u64,
}

impl Creds {
    pub fn new(tokens: Tokens, server: Arc<dyn Renew>, store: Arc<dyn TokenStore>) -> Arc<Creds> {
        Self::with_clock(tokens, server, store, now_unix)
    }

    pub fn with_clock(tokens: Tokens, server: Arc<dyn Renew>, store: Arc<dyn TokenStore>, clock: fn() -> u64) -> Arc<Creds> {
        Arc::new(Creds { tokens: Mutex::new(Some(tokens)), server, store, persist: AtomicBool::new(true), clock })
    }

    /// Runs a request with a valid access token; see the type's docs.
    pub fn call<T>(&self, request: impl Fn(&str) -> Result<T, ApiError>) -> Result<T, ApiError> {
        let access = self.fresh()?;
        match request(&access) {
            Err(ApiError::Unauthorized) => request(&self.renew_after(&access)?),
            other => other,
        }
    }

    /// The access token, refreshed first if it's about to expire.
    fn fresh(&self) -> Result<String, ApiError> {
        let mut tokens = self.lock();
        let current = tokens.as_ref().ok_or(ApiError::Unauthorized)?;
        let expiring = current.claims().is_none_or(|c| c.is_expired((self.clock)()));
        if expiring && current.refresh.is_some() {
            return self.renew(&mut tokens);
        }
        Ok(current.access.clone())
    }

    /// A new access token after `rejected` was refused, unless another thread already got one.
    fn renew_after(&self, rejected: &str) -> Result<String, ApiError> {
        let mut tokens = self.lock();
        match tokens.as_ref() {
            None => Err(ApiError::Unauthorized),
            Some(t) if t.access != rejected => Ok(t.access.clone()),
            Some(_) => self.renew(&mut tokens),
        }
    }

    fn renew(&self, tokens: &mut Option<Tokens>) -> Result<String, ApiError> {
        let Some(refresh) = tokens.as_ref().and_then(|t| t.refresh.clone()) else {
            *tokens = None;
            return Err(ApiError::Unauthorized);
        };
        match self.server.refresh(&refresh) {
            Ok(next) => {
                if self.persist.load(Ordering::SeqCst) {
                    self.store.save(&next.encode());
                }
                let access = next.access.clone();
                *tokens = Some(next);
                Ok(access)
            }
            Err(ApiError::Unauthorized) => {
                *tokens = None;
                Err(ApiError::Unauthorized)
            }
            // Offline: keep the tokens and try again with the next request.
            Err(e) => Err(e),
        }
    }

    /// The user signed out: stop writing to the credential store. Queued requests may still run.
    pub fn detach(&self) {
        self.persist.store(false, Ordering::SeqCst);
    }

    /// Ends the sign-in on the server too. Run it after the requests queued before it.
    pub fn logout(&self) {
        self.detach();
        let ended = self.lock().take();
        if let Some(refresh) = ended.and_then(|t| t.refresh) {
            self.server.logout(&refresh);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Tokens>> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests;
