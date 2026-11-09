//! The signed-in session: the JWT, what it says about the user, and where it's kept between
//! launches (Windows Credential Manager, never a plain file).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

const KEYRING_SERVICE: &str = "todo-notes";
const KEYRING_USER: &str = "session-token";

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub token: String,
    pub email: String,
    /// Unix seconds, from the token's `exp` claim.
    pub expires_at: u64,
}

impl Session {
    /// Reads the claims out of a JWT without verifying it: the server verifies; the client only
    /// needs the email to show and the expiry to avoid sending a token it knows is dead.
    pub fn from_token(token: &str) -> Option<Session> {
        let payload = token.split('.').nth(1)?;
        let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(Session {
            token: token.to_string(),
            email: claims.get("email")?.as_str()?.to_string(),
            expires_at: claims.get("exp")?.as_u64()?,
        })
    }

    /// Expired, or within a minute of it (so a request doesn't race the deadline).
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix + 60 >= self.expires_at
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where the token lives between launches.
pub trait TokenStore {
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str);
    fn clear(&self);
}

/// Windows Credential Manager (Keychain on macOS, Secret Service on Linux).
pub struct Keyring;

impl Keyring {
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()
    }
}

impl TokenStore for Keyring {
    fn load(&self) -> Option<String> {
        Self::entry()?.get_password().ok()
    }

    fn save(&self, token: &str) {
        if let Some(entry) = Self::entry() {
            let _ = entry.set_password(token);
        }
    }

    fn clear(&self) {
        if let Some(entry) = Self::entry() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: &str) -> String {
        format!("eyJhbGciOiJIUzI1NiJ9.{}.sig", URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn reads_email_and_expiry() {
        let s = Session::from_token(&jwt(r#"{"sub":"1","email":"a@b.co","exp":2000}"#)).unwrap();
        assert_eq!(s.email, "a@b.co");
        assert_eq!(s.expires_at, 2000);
        assert!(!s.is_expired(1000));
        assert!(s.is_expired(1950));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Session::from_token("not-a-jwt").is_none());
        assert!(Session::from_token(&jwt(r#"{"sub":"1"}"#)).is_none());
    }
}
