use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

const NOW: u64 = 1_000_000;

fn token(exp: u64) -> String {
    let claims = format!(r#"{{"sub":"u1","email":"a@b.co","exp":{exp}}}"#);
    format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims))
}

/// What `/login` returns: an access token and a refresh token.
fn tokens(exp: u64) -> Tokens {
    Tokens { access: token(exp), refresh: Some("rt_1".into()) }
}

fn me() -> Me {
    Me { id: "u1".into(), email: "a@b.co".into() }
}

fn submit(mode: Mode, email: &str, password: &str) -> Event {
    Event::Submit { mode, email: email.into(), password: password.into() }
}

/// Signed out, then a valid submit: returns the attempt the request carries.
fn submitted(seq: &mut Seq, auth: &mut Auth) -> Attempt {
    let fx: Vec<_> = step(seq, auth, submit(Mode::SignIn, " a@b.co ", "pw")).into_iter().collect();
    match (&*auth, fx.as_slice()) {
        (Auth::Submitting { attempt, .. }, [Effect::Send { email, .. }]) => {
            assert_eq!(email, "a@b.co", "email is trimmed");
            *attempt
        }
        other => panic!("expected Submitting + Send, got {other:?}"),
    }
}

#[test]
fn launch_without_saved_token_shows_form() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let fx = step(&mut seq, &mut auth, Event::Launch { saved: None, now: NOW });
    assert_eq!(auth, Auth::SignedOut { error: None });
    assert!(fx.is_empty());
}

#[test]
fn launch_with_expired_or_garbage_token_clears_it() {
    for saved in [token(NOW - 10), "garbage".to_string()] {
        let (mut seq, mut auth) = (Seq::default(), Auth::default());
        let fx: Vec<_> =
            step(&mut seq, &mut auth, Event::Launch { saved: Some(saved), now: NOW }).into_iter().collect();
        assert_eq!(auth, Auth::SignedOut { error: None });
        assert_eq!(fx, vec![Effect::ClearToken]);
    }
}

#[test]
fn launch_with_good_token_checks_then_signs_in() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let fx: Vec<_> =
        step(&mut seq, &mut auth, Event::Launch { saved: Some(token(NOW + 3600)), now: NOW })
            .into_iter()
            .collect();
    let Auth::Checking { attempt, .. } = auth.clone() else { panic!("{auth:?}") };
    assert!(matches!(fx.as_slice(), [Effect::CheckToken { .. }]));

    step(&mut seq, &mut auth, Event::Checked { attempt, result: Ok(me()) });
    assert!(matches!(auth, Auth::SignedIn { ref session } if session.email == "a@b.co"));
}

#[test]
fn rejected_saved_token_is_cleared_with_notice() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    step(&mut seq, &mut auth, Event::Launch { saved: Some(token(NOW + 3600)), now: NOW });
    let Auth::Checking { attempt, .. } = auth.clone() else { panic!() };
    let fx: Vec<_> = step(
        &mut seq,
        &mut auth,
        Event::Checked { attempt, result: Err(ApiError::Unauthorized) },
    )
    .into_iter()
    .collect();
    assert_eq!(auth, Auth::SignedOut { error: Some(FormError::Api(ApiError::Unauthorized)) });
    assert_eq!(fx, vec![Effect::ClearToken]);
}

#[test]
fn offline_check_keeps_token() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    step(&mut seq, &mut auth, Event::Launch { saved: Some(token(NOW + 3600)), now: NOW });
    let Auth::Checking { attempt, .. } = auth.clone() else { panic!() };
    let fx = step(&mut seq, &mut auth, Event::Checked { attempt, result: Err(ApiError::Unreachable) });
    assert_eq!(auth, Auth::SignedOut { error: Some(FormError::Api(ApiError::Unreachable)) });
    assert!(fx.is_empty(), "an unreachable server says nothing about the token");
}

#[test]
fn validation_errors_never_send() {
    let cases = [
        (Mode::SignIn, "", "pw", FormError::Email("Enter your email.")),
        (Mode::SignIn, "nope", "pw", FormError::Email("That doesn't look like an email address.")),
        (Mode::SignIn, "a@b", "pw", FormError::Email("That doesn't look like an email address.")),
        (Mode::SignIn, "a@b.co", "", FormError::Password("Enter your password.")),
        (Mode::SignUp, "a@b.co", "short", FormError::Password("Use at least 8 characters.")),
    ];
    for (mode, email, password, want) in cases {
        let (mut seq, mut auth) = (Seq::default(), Auth::default());
        let fx = step(&mut seq, &mut auth, submit(mode, email, password));
        assert_eq!(auth, Auth::SignedOut { error: Some(want) }, "{email:?} / {password:?}");
        assert!(fx.is_empty());
    }
}

#[test]
fn successful_sign_in_saves_token_and_forgets_password() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let attempt = submitted(&mut seq, &mut auth);
    let t = tokens(NOW + 900);
    let fx: Vec<_> =
        step(&mut seq, &mut auth, Event::Answered { attempt, result: Ok(t.clone()) }).into_iter().collect();
    assert_eq!(auth, Auth::SignedIn { session: Session { email: "a@b.co".into() } });
    assert_eq!(fx, vec![Effect::Begin(t), Effect::ForgetPassword]);
}

#[test]
fn wrong_password_shows_error_on_password() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let attempt = submitted(&mut seq, &mut auth);
    step(&mut seq, &mut auth, Event::Answered { attempt, result: Err(ApiError::InvalidCredentials) });
    let Auth::SignedOut { error: Some(e) } = &auth else { panic!("{auth:?}") };
    assert!(e.on_password());
    assert_eq!(e.message(), "Wrong email or password.");
}

#[test]
fn double_submit_sends_once() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    submitted(&mut seq, &mut auth);
    let fx = step(&mut seq, &mut auth, submit(Mode::SignIn, "a@b.co", "pw"));
    assert!(fx.is_empty(), "a submit while one is in flight is ignored");
}

#[test]
fn stale_answer_is_ignored() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let first = submitted(&mut seq, &mut auth);
    step(&mut seq, &mut auth, Event::Answered { attempt: first, result: Err(ApiError::Unreachable) });
    let second = submitted(&mut seq, &mut auth);
    // The first request's (late, duplicate) answer arrives while the second is in flight.
    let fx = step(&mut seq, &mut auth, Event::Answered { attempt: first, result: Ok(tokens(NOW + 3600)) });
    assert!(fx.is_empty());
    assert!(matches!(auth, Auth::Submitting { attempt, .. } if attempt == second));
}

#[test]
fn editing_clears_error() {
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    step(&mut seq, &mut auth, submit(Mode::SignIn, "", ""));
    step(&mut seq, &mut auth, Event::Edited);
    assert_eq!(auth, Auth::SignedOut { error: None });
}

#[test]
fn sign_out_and_401_return_to_form_and_clear_token() {
    for (event, error, effects) in [
        // Signing out also ends the sign-in on the server.
        (Event::SignOut, None, vec![Effect::Revoke, Effect::ClearToken]),
        (Event::Unauthorized, Some(FormError::Api(ApiError::Unauthorized)), vec![Effect::ClearToken]),
    ] {
        let (mut seq, mut auth) = (Seq::default(), Auth::default());
        let attempt = submitted(&mut seq, &mut auth);
        step(&mut seq, &mut auth, Event::Answered { attempt, result: Ok(tokens(NOW + 3600)) });
        let fx: Vec<_> = step(&mut seq, &mut auth, event).into_iter().collect();
        assert_eq!(auth, Auth::SignedOut { error });
        assert_eq!(fx, effects);
    }
}

#[test]
fn launch_with_an_expired_access_token_but_a_refresh_token_checks_it() {
    // The check refreshes on the way; only a refused refresh signs the user out.
    let saved = tokens(NOW - 3600).encode();
    let (mut seq, mut auth) = (Seq::default(), Auth::default());
    let fx: Vec<_> = step(&mut seq, &mut auth, Event::Launch { saved: Some(saved), now: NOW }).into_iter().collect();
    assert!(matches!(auth, Auth::Checking { ref session, .. } if session.email == "a@b.co"));
    assert!(matches!(fx.as_slice(), [Effect::CheckToken { tokens: t, .. }] if *t == tokens(NOW - 3600)));
}
