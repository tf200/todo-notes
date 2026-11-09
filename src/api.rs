//! The todo backend's HTTP API. Blocking calls: run them off the UI thread (`rune::Latest`).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::session::{Renew, Tokens};

/// The live server. `TODO_API_URL` points the app elsewhere (the dev server: `DEV_BASE_URL`).
pub const DEFAULT_BASE_URL: &str = "https://todo.loket.site";

/// The dev stack in WSL (`docker compose -f docker-compose.dev.yml`); the live tests use it.
#[cfg_attr(not(test), allow(dead_code))]
pub const DEV_BASE_URL: &str = "http://localhost:8090";

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
    /// The list or task is gone (404 on `/api/*`): deleted elsewhere, or never saved.
    NotFound,
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
            ApiError::NotFound => "That item no longer exists.",
            ApiError::Server(_) => "Something went wrong on the server. Try again.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Me {
    pub id: String,
    pub email: String,
}

/// A todo list (`/api/lists`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
    /// RFC 3339; sorts in creation order as text.
    #[serde(default)]
    pub created_at: String,
}

/// A task in a list (`/api/lists/:id/tasks`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// RFC 3339, when set.
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub created_at: String,
}

/// The fields of a task to change (`PUT /api/tasks/:id`); `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    /// `Some(None)` sends `"due_at": null`, which removes the due date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<Option<String>>,
}

/// A note (`/api/notes`). The body is Markdown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Note {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created_at: String,
    /// RFC 3339; the last content edit (pinning doesn't count).
    #[serde(default)]
    pub updated_at: String,
}

/// The fields of a note to change (`PUT /api/notes/:id`); `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NotePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Deserialize)]
struct TokenBody {
    token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

impl From<TokenBody> for Tokens {
    fn from(b: TokenBody) -> Tokens {
        Tokens { access: b.token, refresh: b.refresh_token }
    }
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

    /// `POST /login`: a session for these credentials.
    pub fn login(&self, email: &str, password: &str) -> Result<Tokens, ApiError> {
        let body = json!({ "email": email, "password": password });
        let (status, mut resp) = self.send(self.agent.post(self.url("/login")).send_json(body))?;
        match status {
            200 => read_json::<TokenBody>(&mut resp).map(Tokens::from),
            401 => Err(ApiError::InvalidCredentials),
            400 => Err(ApiError::BadInput),
            s => Err(ApiError::Server(s)),
        }
    }

    /// `POST /register`: creates the account and returns a session, so the user is signed in.
    pub fn register(&self, email: &str, password: &str) -> Result<Tokens, ApiError> {
        let body = json!({ "email": email, "password": password });
        let (status, mut resp) =
            self.send(self.agent.post(self.url("/register")).send_json(body))?;
        match status {
            201 | 200 => read_json::<TokenBody>(&mut resp).map(Tokens::from),
            409 => Err(ApiError::EmailTaken),
            400 => Err(ApiError::BadInput),
            s => Err(ApiError::Server(s)),
        }
    }

    /// `POST /refresh`: the next access and refresh token. `Unauthorized` when the sign-in has
    /// ended (revoked, expired, or the refresh token was already used).
    pub fn refresh(&self, refresh: &str) -> Result<Tokens, ApiError> {
        let body = json!({ "refresh_token": refresh });
        let (status, mut resp) = self.send(self.agent.post(self.url("/refresh")).send_json(body))?;
        match status {
            200 => read_json::<TokenBody>(&mut resp).map(Tokens::from),
            401 | 400 => Err(ApiError::Unauthorized),
            s => Err(ApiError::Server(s)),
        }
    }

    /// `POST /logout`: revokes the sign-in this refresh token belongs to.
    pub fn logout(&self, refresh: &str) -> Result<(), ApiError> {
        let body = json!({ "refresh_token": refresh });
        match self.send(self.agent.post(self.url("/logout")).send_json(body))?.0 {
            200 | 204 => Ok(()),
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

    /// `GET /api/lists`: the user's lists, oldest first.
    pub fn lists(&self, token: &str) -> Result<Vec<TaskList>, ApiError> {
        let req = self.agent.get(self.url("/api/lists")).header("Authorization", bearer(token));
        let mut lists = self.read_vec::<TaskList>(req.call())?;
        lists.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(lists)
    }

    /// `POST /api/lists`.
    pub fn create_list(&self, token: &str, title: &str) -> Result<TaskList, ApiError> {
        let req = self.agent.post(self.url("/api/lists")).header("Authorization", bearer(token));
        let (status, mut resp) = self.send(req.send_json(json!({ "title": title })))?;
        match status {
            200 | 201 => read_json(&mut resp),
            s => Err(api_status(s)),
        }
    }

    /// `PUT /api/lists/:id`.
    pub fn rename_list(&self, token: &str, id: &str, title: &str) -> Result<(), ApiError> {
        let req = self.agent.put(self.url(&format!("/api/lists/{id}"))).header("Authorization", bearer(token));
        self.expect_ok(req.send_json(json!({ "title": title })))
    }

    /// `DELETE /api/lists/:id` (the server deletes its tasks too).
    pub fn delete_list(&self, token: &str, id: &str) -> Result<(), ApiError> {
        let req = self.agent.delete(self.url(&format!("/api/lists/{id}"))).header("Authorization", bearer(token));
        self.expect_ok(req.call())
    }

    /// `GET /api/lists/:id/tasks`: the list's tasks, oldest first.
    pub fn tasks(&self, token: &str, list: &str) -> Result<Vec<Task>, ApiError> {
        let req = self.agent.get(self.url(&format!("/api/lists/{list}/tasks"))).header("Authorization", bearer(token));
        let mut tasks = self.read_vec::<Task>(req.call())?;
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(tasks)
    }

    /// `POST /api/lists/:id/tasks`.
    pub fn create_task(&self, token: &str, list: &str, title: &str) -> Result<Task, ApiError> {
        let req = self.agent.post(self.url(&format!("/api/lists/{list}/tasks"))).header("Authorization", bearer(token));
        let (status, mut resp) = self.send(req.send_json(json!({ "title": title })))?;
        match status {
            200 | 201 => read_json(&mut resp),
            s => Err(api_status(s)),
        }
    }

    /// `PUT /api/tasks/:id` with only the fields that changed.
    pub fn update_task(&self, token: &str, id: &str, patch: &TaskPatch) -> Result<(), ApiError> {
        let req = self.agent.put(self.url(&format!("/api/tasks/{id}"))).header("Authorization", bearer(token));
        self.expect_ok(req.send_json(patch))
    }

    /// `DELETE /api/tasks/:id`.
    pub fn delete_task(&self, token: &str, id: &str) -> Result<(), ApiError> {
        let req = self.agent.delete(self.url(&format!("/api/tasks/{id}"))).header("Authorization", bearer(token));
        self.expect_ok(req.call())
    }

    /// `GET /api/notes`: the user's notes, most recently edited first.
    pub fn notes(&self, token: &str) -> Result<Vec<Note>, ApiError> {
        let req = self.agent.get(self.url("/api/notes")).header("Authorization", bearer(token));
        self.read_vec::<Note>(req.call())
    }

    /// `POST /api/notes`.
    pub fn create_note(&self, token: &str, title: &str, body: &str) -> Result<Note, ApiError> {
        let req = self.agent.post(self.url("/api/notes")).header("Authorization", bearer(token));
        let (status, mut resp) = self.send(req.send_json(json!({ "title": title, "body": body })))?;
        match status {
            200 | 201 => read_json(&mut resp),
            s => Err(api_status(s)),
        }
    }

    /// `PUT /api/notes/:id` with only the fields that changed; returns the saved note.
    pub fn update_note(&self, token: &str, id: &str, patch: &NotePatch) -> Result<Note, ApiError> {
        let req = self.agent.put(self.url(&format!("/api/notes/{id}"))).header("Authorization", bearer(token));
        let (status, mut resp) = self.send(req.send_json(patch))?;
        match status {
            200 => read_json(&mut resp),
            s => Err(api_status(s)),
        }
    }

    /// `DELETE /api/notes/:id`.
    pub fn delete_note(&self, token: &str, id: &str) -> Result<(), ApiError> {
        let req = self.agent.delete(self.url(&format!("/api/notes/{id}"))).header("Authorization", bearer(token));
        self.expect_ok(req.call())
    }

    /// A JSON array body; the server sends `null` for none.
    fn read_vec<T: for<'de> Deserialize<'de>>(
        &self,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Vec<T>, ApiError> {
        let (status, mut resp) = self.send(result)?;
        match status {
            200 => read_json::<Option<Vec<T>>>(&mut resp).map(Option::unwrap_or_default),
            s => Err(api_status(s)),
        }
    }

    fn expect_ok(&self, result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(), ApiError> {
        match self.send(result)?.0 {
            200 | 201 | 204 => Ok(()),
            s => Err(api_status(s)),
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

impl Renew for Api {
    fn refresh(&self, refresh: &str) -> Result<Tokens, ApiError> {
        Api::refresh(self, refresh)
    }
    fn logout(&self, refresh: &str) {
        let _ = Api::logout(self, refresh);
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// An unexpected status from an `/api/*` route.
fn api_status(status: u16) -> ApiError {
    match status {
        401 => ApiError::Unauthorized,
        404 => ApiError::NotFound,
        400 => ApiError::BadInput,
        s => ApiError::Server(s),
    }
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
