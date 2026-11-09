use std::sync::Mutex;

use super::*;

const NOW: u64 = 1_000_000;

fn jwt(name: &str, exp: u64) -> String {
    let claims = format!(r#"{{"sub":"1","email":"a@b.co","exp":{exp},"n":"{name}"}}"#);
    format!("eyJhbGciOiJIUzI1NiJ9.{}.sig", URL_SAFE_NO_PAD.encode(claims))
}

fn pair(name: &str, exp: u64, refresh: &str) -> Tokens {
    Tokens { access: jwt(name, exp), refresh: Some(refresh.into()) }
}

#[test]
fn reads_email_and_expiry() {
    let c = Claims::read(&jwt("a", 2000)).unwrap();
    assert_eq!(c.email, "a@b.co");
    assert_eq!(c.expires_at, 2000);
    assert!(!c.is_expired(1000));
    assert!(c.is_expired(1950));
}

#[test]
fn rejects_garbage() {
    assert!(Claims::read("not-a-jwt").is_none());
    assert!(Tokens::decode("not-a-jwt").is_none());
    assert!(Tokens::decode(r#"{"access":"nope"}"#).is_none());
}

#[test]
fn stored_tokens_round_trip_and_old_bare_tokens_still_load() {
    let t = pair("a", NOW + 900, "rt_1");
    assert_eq!(Tokens::decode(&t.encode()), Some(t));
    let legacy = jwt("old", NOW + 900);
    assert_eq!(Tokens::decode(&legacy), Some(Tokens { access: legacy.clone(), refresh: None }));
}

#[test]
fn an_expired_access_token_is_still_usable_with_a_refresh_token() {
    assert!(pair("a", NOW - 10, "rt").usable(NOW));
    assert!(!Tokens { access: jwt("a", NOW - 10), refresh: None }.usable(NOW));
    assert!(Tokens { access: jwt("a", NOW + 900), refresh: None }.usable(NOW));
}

/// A server that hands out `a1`, `a2`… for `rt_0`, `rt_1`… and accepts each refresh token once.
#[derive(Default)]
struct Server {
    issued: Mutex<u32>,
    answer: Mutex<Option<ApiError>>,
    logged_out: Mutex<Vec<String>>,
}

impl Renew for Server {
    fn refresh(&self, refresh: &str) -> Result<Tokens, ApiError> {
        if let Some(e) = self.answer.lock().unwrap().clone() {
            return Err(e);
        }
        let mut n = self.issued.lock().unwrap();
        assert_eq!(refresh, format!("rt_{n}"), "a spent refresh token was sent");
        *n += 1;
        Ok(pair(&format!("a{n}"), NOW + 900, &format!("rt_{n}")))
    }
    fn logout(&self, refresh: &str) {
        self.logged_out.lock().unwrap().push(refresh.into());
    }
}

#[derive(Default)]
struct Store(Mutex<Vec<String>>);

impl TokenStore for Store {
    fn load(&self) -> Option<String> {
        self.0.lock().unwrap().last().cloned()
    }
    fn save(&self, token: &str) {
        self.0.lock().unwrap().push(token.into());
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn creds(exp: u64) -> (Arc<Creds>, Arc<Server>, Arc<Store>) {
    let (server, store) = (Arc::new(Server::default()), Arc::new(Store::default()));
    let c = Creds::with_clock(pair("a0", exp, "rt_0"), server.clone(), store.clone(), || NOW);
    (c, server, store)
}

/// Which access token a request carried: the `n` claim.
fn name(access: &str) -> String {
    let payload = access.split('.').nth(1).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    v["n"].as_str().unwrap().to_string()
}

#[test]
fn a_live_token_is_used_as_it_is() {
    let (c, server, store) = creds(NOW + 900);
    assert_eq!(c.call(|t| Ok(name(t))), Ok("a0".to_string()));
    assert_eq!(*server.issued.lock().unwrap(), 0);
    assert!(store.load().is_none());
}

#[test]
fn a_token_about_to_expire_is_refreshed_first_and_saved() {
    let (c, _, store) = creds(NOW + 30);
    assert_eq!(c.call(|t| Ok(name(t))), Ok("a1".to_string()));
    let saved = Tokens::decode(&store.load().unwrap()).unwrap();
    assert_eq!(saved.refresh.as_deref(), Some("rt_1"));
    // The next request uses the new pair without refreshing again.
    assert_eq!(c.call(|t| Ok(name(t))), Ok("a1".to_string()));
}

#[test]
fn a_401_refreshes_and_retries_once() {
    let (c, _, _) = creds(NOW + 900);
    let tries = Mutex::new(vec![]);
    let result = c.call(|t| {
        tries.lock().unwrap().push(name(t));
        if name(t) == "a0" { Err(ApiError::Unauthorized) } else { Ok("saved") }
    });
    assert_eq!(result, Ok("saved"));
    assert_eq!(*tries.lock().unwrap(), ["a0", "a1"]);
}

#[test]
fn a_401_after_refreshing_is_final() {
    let (c, _, _) = creds(NOW + 900);
    let result: Result<(), _> = c.call(|_| Err(ApiError::Unauthorized));
    assert_eq!(result, Err(ApiError::Unauthorized));
}

#[test]
fn a_refused_refresh_ends_the_session() {
    let (c, server, _) = creds(NOW + 30);
    *server.answer.lock().unwrap() = Some(ApiError::Unauthorized);
    assert_eq!(c.call(|t| Ok(name(t))), Err(ApiError::Unauthorized));
    // Later requests fail at once; the server isn't asked again.
    *server.answer.lock().unwrap() = None;
    assert_eq!(c.call(|t| Ok(name(t))), Err(ApiError::Unauthorized));
    assert_eq!(*server.issued.lock().unwrap(), 0);
}

#[test]
fn offline_keeps_the_tokens_for_the_next_try() {
    let (c, server, _) = creds(NOW + 30);
    *server.answer.lock().unwrap() = Some(ApiError::Unreachable);
    assert_eq!(c.call(|t| Ok(name(t))), Err(ApiError::Unreachable));
    *server.answer.lock().unwrap() = None;
    assert_eq!(c.call(|t| Ok(name(t))), Ok("a1".to_string()));
}

#[test]
fn a_thread_that_waited_on_a_refresh_uses_its_result() {
    let (c, server, _) = creds(NOW + 900);
    // One thread has `a0` refused and refreshes to `a1`…
    c.call(|t| if name(t) == "a0" { Err(ApiError::Unauthorized) } else { Ok(()) }).unwrap();
    // …while another, also refused with `a0`, was waiting: it takes `a1` instead of spending
    // the refresh token a second time.
    assert_eq!(c.renew_after(&jwt("a0", NOW + 900)).map(|t| name(&t)), Ok("a1".to_string()));
    assert_eq!(*server.issued.lock().unwrap(), 1, "no second refresh");
}

#[test]
fn after_sign_out_refreshes_are_not_saved_and_logout_revokes() {
    let (c, server, store) = creds(NOW + 30);
    c.detach();
    // A save queued before sign-out still goes through, refreshing on the way.
    assert_eq!(c.call(|t| Ok(name(t))), Ok("a1".to_string()));
    assert!(store.load().is_none(), "nothing written after sign-out");
    c.logout();
    assert_eq!(*server.logged_out.lock().unwrap(), ["rt_1"]);
    assert_eq!(c.call(|t| Ok(name(t))), Err(ApiError::Unauthorized));
}
