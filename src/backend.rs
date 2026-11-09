//! Runs the todo model's requests on one worker thread, in the order they were made.
//!
//! One thread, in order, so optimistic edits reach the server as the user made them: a task
//! added and ticked before its create returns is created first, then updated. Items made on
//! this side carry a temporary id (`tmp-…`) until the server answers; the worker swaps it for
//! the server's id in every later request.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use rune::Waker;

use crate::api::Api;
use crate::todos::{Op, Reply, is_temp};

pub struct Backend {
    /// `None` once [`Backend::finish`] has closed the queue.
    send: Option<Sender<Job>>,
    replies: Receiver<(u64, Reply)>,
    /// Signalled when the worker has run every queued request and stopped.
    stopped: Receiver<()>,
}

struct Job {
    epoch: u64,
    token: String,
    op: Op,
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
                    if let Some(r) = run(&api, &job.token, job.op, &mut ids) {
                        // The app may be gone (closing); keep going so queued saves still land.
                        if reply.send((job.epoch, r)).is_ok() {
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
    pub fn run(&self, epoch: u64, token: &str, op: Op) {
        if let Some(send) = &self.send {
            let _ = send.send(Job { epoch, token: token.to_string(), op });
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
    use crate::todos::{Effect, Event, Load, Todos, step};

    struct Rig {
        api: Api,
        token: String,
        backend: Backend,
        todos: Todos,
    }

    impl Rig {
        fn send(&mut self, e: Event) {
            for fx in step(&mut self.todos, e) {
                match fx {
                    Effect::Run(op) => self.backend.run(self.todos.epoch, &self.token, op),
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
        let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| crate::api::DEFAULT_BASE_URL.into());
        let api = Api::new(base);
        let token = api
            .login(&std::env::var("TODO_DEV_EMAIL").unwrap(), &std::env::var("TODO_DEV_PASSWORD").unwrap())
            .expect("dev login");
        let backend = Backend::start(api.clone(), Arc::new(|| {}));
        let mut rig = Rig { api, token, backend, todos: Todos::default() };
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
        let saved = rig.api.tasks(&rig.token, &list).unwrap();
        let summary: Vec<_> = saved.iter().map(|t| (t.title.as_str(), t.done)).collect();
        assert_eq!(summary, [("Ticked before it was saved", true), ("Left open", false)]);

        // Deleting the list deletes its tasks on the server.
        rig.send(Event::DeleteList(list.clone()));
        rig.until(|t| t.lists().iter().all(|l| l.id != list));
        std::thread::sleep(Duration::from_millis(500));
        assert!(rig.api.lists(&rig.token).unwrap().iter().all(|l| l.id != list));
        assert_eq!(rig.api.tasks(&rig.token, &list), Err(crate::api::ApiError::NotFound));
    }

    #[test]
    #[ignore = "needs the backend running and the dev account"]
    fn a_note_typed_and_pinned_before_its_create_returns_is_saved() {
        use crate::notes::{self, Event as N, Notes};
        let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| crate::api::DEFAULT_BASE_URL.into());
        let api = Api::new(base);
        let token = api
            .login(&std::env::var("TODO_DEV_EMAIL").unwrap(), &std::env::var("TODO_DEV_PASSWORD").unwrap())
            .expect("dev login");
        let backend = Backend::start(api.clone(), Arc::new(|| {}));
        let mut n = Notes::default();
        let send = |n: &mut Notes, e: N| {
            for fx in notes::step(n, e) {
                if let Effect::Run(op) = fx {
                    backend.run(0, &token, op);
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
                                backend.run(0, &token, op);
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

        let saved = api.notes(&token).unwrap().into_iter().find(|x| x.id == id).expect("on the server");
        assert_eq!((saved.title.as_str(), saved.body.as_str(), saved.pinned), ("E2E note", "- [ ] one", true));
        assert_eq!(n.notice, None);

        send(&mut n, N::Delete(id.clone()));
        std::thread::sleep(Duration::from_millis(500));
        assert!(api.notes(&token).unwrap().iter().all(|x| x.id != id));
    }
}

/// Carries out one request. `ids` maps temporary ids to the server's. Returns `None` for a
/// request about an item whose create failed: the model already took it back.
fn run(api: &Api, token: &str, op: Op, ids: &mut HashMap<String, String>) -> Option<Reply> {
    let id = |ids: &HashMap<String, String>, id: &str| -> Option<String> {
        if is_temp(id) { ids.get(id).cloned() } else { Some(id.to_string()) }
    };
    Some(match op {
        Op::LoadLists => Reply::Lists(api.lists(token)),
        Op::LoadTasks { list } => {
            let real = id(ids, &list)?;
            Reply::Tasks { list, result: api.tasks(token, &real) }
        }
        Op::CreateList { tmp, title } => {
            let result = api.create_list(token, &title);
            if let Ok(l) = &result {
                ids.insert(tmp.clone(), l.id.clone());
            }
            Reply::ListCreated { tmp, result }
        }
        Op::RenameList { id: list, title } => Reply::Saved(api.rename_list(token, &id(ids, &list)?, &title)),
        Op::DeleteList { id: list } => Reply::Saved(api.delete_list(token, &id(ids, &list)?)),
        Op::CreateTask { tmp, list, title } => {
            let result = api.create_task(token, &id(ids, &list)?, &title);
            if let Ok(t) = &result {
                ids.insert(tmp.clone(), t.id.clone());
            }
            Reply::TaskCreated { tmp, list, result }
        }
        Op::UpdateTask { id: task, patch } => Reply::Saved(api.update_task(token, &id(ids, &task)?, &patch)),
        Op::DeleteTask { id: task } => Reply::Saved(api.delete_task(token, &id(ids, &task)?)),
        Op::LoadNotes => Reply::Notes(api.notes(token)),
        Op::CreateNote { tmp, title, body } => {
            let result = api.create_note(token, &title, &body);
            if let Ok(n) = &result {
                ids.insert(tmp.clone(), n.id.clone());
            }
            Reply::NoteCreated { tmp, result }
        }
        Op::UpdateNote { id: note, patch } => Reply::NoteSaved(api.update_note(token, &id(ids, &note)?, &patch)),
        Op::DeleteNote { id: note } => Reply::NoteDeleted(api.delete_note(token, &id(ids, &note)?)),
    })
}
