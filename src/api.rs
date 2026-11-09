//! The todo backend's HTTP API. Blocking calls: run them off the UI thread (`rune::Latest`).

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

pub const DEFAULT_BASE_URL: &str = "http://localhost:8090";

/// What went wrong, in terms the UI can show. Raw server text never reaches the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// Wrong email or password (401 on `/login`).
    InvalidCredentials,
    /// The email already has an account (409 on `/register`).
    EmailTaken,
    /// The server rejected the input (400).
    BadInput,
    /// The token is missing, expired or revoked (401 on `/api/*`).
    Unauthorized,
    /// No answer: the server is down, the port is wrong, or the request timed out.
    Unreachable,
    /// Anything else (5xx, an unexpected body).
    Server(u16),
}

impl ApiError {
    pub fn message(&self) -> &'static str {
        match self {
            ApiError::InvalidCredentials => "Wrong email or password.",
            ApiError::EmailTaken => "An account with this email already exists.",
            ApiError::BadInput => "Check the email and password and try again.",
            ApiError::Unauthorized => "Your session has expired. Sign in again.",
            ApiError::Unreachable => "Can't reach the server. Check that it's running.",
            ApiError::Server(_) => "Something went wrong on the server. Try again.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Me {
    pub id: String,
    pub email: String,
}

#[derive(Deserialize)]
struct TokenBody {
    token: String,
}

#[derive(Clone)]
pub struct Api {
    base: String,
    agent: ureq::Agent,
}

impl Api {
    pub fn new(base: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
            .into();
        Api { base: base.into().trim_end_matches('/').to_string(), agent }
    }

    /// `POST /login`: a JWT for these credentials.
    pub fn login(&self, email: &str, password: &str) -> Result<String, ApiError> {
        let body = json!({ "email": email, "password": password });
        let (status, mut resp) = self.send(self.agent.post(self.url("/login")).send_json(body))?;
        match status {
            200 => read_json::<TokenBody>(&mut resp).map(|b| b.token),
            401 => Err(ApiError::InvalidCredentials),
            400 => Err(ApiError::BadInput),
            s => Err(ApiError::Server(s)),
        }
    }

    /// `POST /register`: creates the account and returns a JWT, so the user is signed in.
    pub fn register(&self, email: &str, password: &str) -> Result<String, ApiError> {
        let body = json!({ "email": email, "password": password });
        let (status, mut resp) =
            self.send(self.agent.post(self.url("/register")).send_json(body))?;
        match status {
            201 | 200 => read_json::<TokenBody>(&mut resp).map(|b| b.token),
            409 => Err(ApiError::EmailTaken),
            400 => Err(ApiError::BadInput),
            s => Err(ApiError::Server(s)),
        }
    }

    /// `GET /api/me`: who this token belongs to, or `Unauthorized` when it's no longer valid.
    pub fn me(&self, token: &str) -> Result<Me, ApiError> {
        let req = self.agent.get(self.url("/api/me")).header("Authorization", bearer(token));
        let (status, mut resp) = self.send(req.call())?;
        match status {
            200 => read_json::<Me>(&mut resp),
            401 => Err(ApiError::Unauthorized),
            s => Err(ApiError::Server(s)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn send(
        &self,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<(u16, ureq::http::Response<ureq::Body>), ApiError> {
        // With `http_status_as_error(false)` every error here is transport: refused, DNS, timeout.
        let resp = result.map_err(|_| ApiError::Unreachable)?;
        Ok((resp.status().as_u16(), resp))
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_port_is_unreachable() {
        // Nothing listens on the discard port; the connection is refused.
        let api = Api::new("http://127.0.0.1:9");
        assert_eq!(api.login("a@b.co", "pw"), Err(ApiError::Unreachable));
        assert_eq!(api.me("token"), Err(ApiError::Unreachable));
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(
    resp: &mut ureq::http::Response<ureq::Body>,
) -> Result<T, ApiError> {
    let status = resp.status().as_u16();
    resp.body_mut().read_json::<T>().map_err(|_| ApiError::Server(status))
}
