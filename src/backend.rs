//! Runs the todo model's requests on one worker thread, in the order they were made.
//!
//! One thread, in order, so optimistic edits reach the server as the user made them: a task
//! added and ticked before its create returns is created first, then updated. Items made on
//! this side carry a temporary id (`tmp-…`) until the server answers; the worker swaps it for
//! the server's id in every later request.
//!
//! Each request carries the [`Creds`] of the sign-in that made it, so a request queued before
//! a sign-out still goes out as that user, refreshing the access token if it has to.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use rune::Waker;

use crate::api::Api;
use crate::session::Creds;
use crate::todos::{Op, Reply, is_temp};

pub struct Backend {
    /// `None` once [`Backend::finish`] has closed the queue.
    send: Option<Sender<Job>>,
    replies: Receiver<(u64, Reply)>,
    /// Signalled when the worker has run every queued request and stopped.
    stopped: Receiver<()>,
}

enum Job {
    Run { epoch: u64, creds: Arc<Creds>, op: Op },
    /// End a sign-in on the server, after everything queued before it.
    Logout(Arc<Creds>),
}

impl Backend {
    /// Starts the worker; `waker` asks the window for a frame when a reply lands.
    pub fn start(api: Api, waker: Waker) -> Backend {
        let (send, jobs) = channel::<Job>();
        let (reply, replies) = channel();
        let (stop, stopped) = channel();
        std::thread::Builder::new()
            .name("todo-backend".into())
            .spawn(move || {
                let mut ids = HashMap::new();
                // Ends when the queue closes: `finish`, or the app dropping its `Backend`.
                for job in jobs {
                    let (epoch, creds, op) = match job {
                        Job::Run { epoch, creds, op } => (epoch, creds, op),
                        Job::Logout(creds) => {
                            creds.logout();
                            continue;
                        }
                    };
                    if let Some(r) = run(&api, &creds, op, &mut ids) {
                        // The app may be gone (closing); keep going so queued saves still land.
                        if reply.send((epoch, r)).is_ok() {
                            waker();
                        }
                    }
                }
                let _ = stop.send(());
            })
            .expect("spawn the backend thread");
        Backend { send: Some(send), replies, stopped }
    }

    /// Queues `op`, tagged with the sign-in it belongs to.
    pub fn run(&self, epoch: u64, creds: &Arc<Creds>, op: Op) {
        if let Some(send) = &self.send {
            let _ = send.send(Job::Run { epoch, creds: creds.clone(), op });
        }
    }

    /// Queues the end of a sign-in: requests already queued for it go out first.
    pub fn logout(&self, creds: &Arc<Creds>) {
        match &self.send {
            Some(send) => {
                let _ = send.send(Job::Logout(creds.clone()));
            }
            None => creds.logout(),
        }
    }

    /// Closes the queue and waits, up to `limit`, for the requests already in it to reach the
    /// server: the window is closing and the last edits shouldn't be lost.
    pub fn finish(&mut self, limit: std::time::Duration) {
        self.send = None;
        let _ = self.stopped.recv_timeout(limit);
    }

    /// The next finished request, if any.
    pub fn take(&self) -> Option<(u64, Reply)> {
        self.replies.try_recv().ok()
    }
}

#[cfg(test)]
mod live {
    //! Against a running server, with the dev account: `cargo test -- --ignored`, with
    //! `TODO_DEV_EMAIL` and `TODO_DEV_PASSWORD` set (see `.dev/credentials.env`).
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::session::{NoStore, Tokens};
    use crate::todos::{Effect, Event, Load, Todos, step};

    struct Rig {
        api: Api,
        creds: Arc<Creds>,
        backend: Backend,
        todos: Todos,
    }

    /// Signs in with the dev account: the tokens, and the API to use them with.
    fn sign_in() -> (Api, Tokens) {
        let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| crate::api::DEV_BASE_URL.into());
        let api = Api::new(base);
        let tokens = api
            .login(&std::env::var("TODO_DEV_EMAIL").unwrap(), &std::env::var("TODO_DEV_PASSWORD").unwrap())
            .expect("dev login");
        (api, tokens)
    }

    fn creds(api: &Api, tokens: Tokens) -> Arc<Creds> {
        Creds::new(tokens, Arc::new(api.clone()), Arc::new(NoStore))
    }

    impl Rig {
        fn send(&mut self, e: Event) {
            for fx in step(&mut self.todos, e) {
                match fx {
                    Effect::Run(op) => self.backend.run(self.todos.epoch, &self.creds, op),
                    Effect::Unauthorized => panic!("unauthorized"),
                }
            }
        }
        /// Feeds replies to the model until `done` holds.
        fn until(&mut self, done: impl Fn(&Todos) -> bool) {
            let start = Instant::now();
            while !done(&self.todos) {
                assert!(start.elapsed() < Duration::from_secs(15), "timed out: {:?}", self.todos);
                match self.backend.take() {
                    Some((_, r)) => self.send(Event::Reply(r)),
                    None => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
    }

    #[test]
    #[ignore = "needs the backend running and the dev account"]
    fn optimistic_edits_reach_the_server_in_order() {
        let (api, tokens) = sign_in();
        let backend = Backend::start(api.clone(), Arc::new(|| {}));
        let mut rig = Rig { creds: creds(&api, tokens), api, backend, todos: Todos::default() };
        rig.send(Event::Open);
        rig.until(|t| matches!(t.lists, Load::Ready(_)));

        // A new list, a task in it and a tick, all before the server has answered anything.
        rig.send(Event::CreateList("E2E check".into()));
        rig.send(Event::AddTask("Ticked before it was saved".into()));
        rig.send(Event::AddTask("Left open".into()));
        let tmp = rig.todos.selected.clone().unwrap();
        let first = match rig.todos.tasks_of(&tmp) {
            Some(Load::Ready(t)) => t[0].id.clone(),
            other => panic!("{other:?}"),
        };
        rig.send(Event::SetDone { id: first, done: true });
        rig.until(|t| t.selected.as_deref().is_some_and(|s| !is_temp(s)) && t.tasks.values().all(|l| match l {
            Load::Ready(ts) => ts.iter().all(|x| !is_temp(&x.id)),
            _ => true,
        }));
        assert_eq!(rig.todos.notice, None);

        // What the server has.
        let list = rig.todos.selected.clone().unwrap();
        // The worker runs in order: once this load answers, the tick before it is saved.
        rig.send(Event::Reload);
        std::thread::sleep(Duration::from_millis(300));
        let saved = rig.creds.call(|t| rig.api.tasks(t, &list)).unwrap();
        let summary: Vec<_> = saved.iter().map(|t| (t.title.as_str(), t.done)).collect();
        assert_eq!(summary, [("Ticked before it was saved", true), ("Left open", false)]);

        // Deleting the list deletes its tasks on the server.
        rig.send(Event::DeleteList(list.clone()));
        rig.until(|t| t.lists().iter().all(|l| l.id != list));
        std::thread::sleep(Duration::from_millis(500));
        assert!(rig.creds.call(|t| rig.api.lists(t)).unwrap().iter().all(|l| l.id != list));
        assert_eq!(rig.creds.call(|t| rig.api.tasks(t, &list)), Err(crate::api::ApiError::NotFound));
    }

    #[test]
    #[ignore = "needs the backend running and the dev account"]
    fn a_note_typed_and_pinned_before_its_create_returns_is_saved() {
        use crate::notes::{self, Event as N, Notes};
        let (api, tokens) = sign_in();
        let creds = creds(&api, tokens);
        let token = &creds;
        let backend = Backend::start(api.clone(), Arc::new(|| {}));
        let mut n = Notes::default();
        let send = |n: &mut Notes, e: N| {
            for fx in notes::step(n, e) {
                if let Effect::Run(op) = fx {
                    backend.run(0, token, op);
                }
            }
        };
        let until = |n: &mut Notes, done: &dyn Fn(&Notes) -> bool| {
            let start = Instant::now();
            while !done(n) {
                assert!(start.elapsed() < Duration::from_secs(15), "timed out: {n:?}");
                match backend.take() {
                    Some((_, r)) => {
                        for fx in notes::step(n, N::Reply(r)) {
                            if let Effect::Run(op) = fx {
                                backend.run(0, token, op);
                            }
                        }
                    }
                    None => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        };
        send(&mut n, N::Open);
        until(&mut n, &|n| matches!(n.notes, Load::Ready(_)));

        send(&mut n, N::New);
        let tmp = n.open.clone().unwrap();
        send(&mut n, N::Edit { id: tmp.clone(), title: "E2E note".into(), body: "- [ ] one".into() });
        send(&mut n, N::SetPinned { id: tmp, pinned: true });
        until(&mut n, &|n| n.open.as_deref().is_some_and(|o| !is_temp(o)));
        let id = n.open.clone().unwrap();
        std::thread::sleep(Duration::from_millis(500));

        let saved = creds.call(|t| api.notes(t)).unwrap().into_iter().find(|x| x.id == id).expect("on the server");
        assert_eq!((saved.title.as_str(), saved.body.as_str(), saved.pinned), ("E2E note", "- [ ] one", true));
        assert_eq!(n.notice, None);

        send(&mut n, N::Delete(id.clone()));
        std::thread::sleep(Duration::from_millis(500));
        assert!(creds.call(|t| api.notes(t)).unwrap().iter().all(|x| x.id != id));
    }

    #[test]
    #[ignore = "needs the backend running and the dev account"]
    fn a_refused_access_token_is_refreshed_and_the_request_retried() {
        let (api, tokens) = sign_in();
        let Some(refresh) = tokens.refresh.clone() else { panic!("the server gave no refresh token") };
        // An access token the server will refuse, with a good refresh token.
        let forged = Tokens { access: format!("{}x", tokens.access), refresh: Some(refresh.clone()) };
        let c = creds(&api, forged);
        let me = c.call(|t| api.me(t)).expect("refreshed and retried");
        assert_eq!(me.email, std::env::var("TODO_DEV_EMAIL").unwrap().to_lowercase());

        // Signing out (queued behind any saves) ends the sign-in on the server, the refresh
        // token spent above included.
        let mut backend = Backend::start(api.clone(), Arc::new(|| {}));
        backend.logout(&c);
        backend.finish(Duration::from_secs(5));
        assert_eq!(c.call(|t| api.me(t)), Err(crate::api::ApiError::Unauthorized), "signed out here");
        assert_eq!(api.refresh(&refresh).err(), Some(crate::api::ApiError::Unauthorized), "revoked on the server");
    }
}

/// Carries out one request. `ids` maps temporary ids to the server's. Returns `None` for a
/// request about an item whose create failed: the model already took it back.
fn run(api: &Api, creds: &Creds, op: Op, ids: &mut HashMap<String, String>) -> Option<Reply> {
    let id = |ids: &HashMap<String, String>, id: &str| -> Option<String> {
        if is_temp(id) { ids.get(id).cloned() } else { Some(id.to_string()) }
    };
    Some(match op {
        Op::LoadLists => Reply::Lists(creds.call(|t| api.lists(t))),
        Op::LoadTasks { list } => {
            let real = id(ids, &list)?;
            Reply::Tasks { list, result: creds.call(|t| api.tasks(t, &real)) }
        }
        Op::CreateList { tmp, title } => {
            let result = creds.call(|t| api.create_list(t, &title));
            if let Ok(l) = &result {
                ids.insert(tmp.clone(), l.id.clone());
            }
            Reply::ListCreated { tmp, result }
        }
        Op::RenameList { id: list, title } => {
            let list = id(ids, &list)?;
            Reply::Saved(creds.call(|t| api.rename_list(t, &list, &title)))
        }
        Op::DeleteList { id: list } => {
            let list = id(ids, &list)?;
            Reply::Saved(creds.call(|t| api.delete_list(t, &list)))
        }
        Op::CreateTask { tmp, list, title } => {
            let real = id(ids, &list)?;
            let result = creds.call(|t| api.create_task(t, &real, &title));
            if let Ok(t) = &result {
                ids.insert(tmp.clone(), t.id.clone());
            }
            Reply::TaskCreated { tmp, list, result }
        }
        Op::UpdateTask { id: task, patch } => {
            let task = id(ids, &task)?;
            Reply::Saved(creds.call(|t| api.update_task(t, &task, &patch)))
        }
        Op::DeleteTask { id: task } => {
            let task = id(ids, &task)?;
            Reply::Saved(creds.call(|t| api.delete_task(t, &task)))
        }
        Op::LoadNotes => Reply::Notes(creds.call(|t| api.notes(t))),
        Op::CreateNote { tmp, title, body } => {
            let result = creds.call(|t| api.create_note(t, &title, &body));
            if let Ok(n) = &result {
                ids.insert(tmp.clone(), n.id.clone());
            }
            Reply::NoteCreated { tmp, result }
        }
        Op::UpdateNote { id: note, patch } => {
            let note = id(ids, &note)?;
            Reply::NoteSaved(creds.call(|t| api.update_note(t, &note, &patch)))
        }
        Op::DeleteNote { id: note } => {
            let note = id(ids, &note)?;
            Reply::NoteDeleted(creds.call(|t| api.delete_note(t, &note)))
        }
    })
}
