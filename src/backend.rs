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
    send: Sender<Job>,
    replies: Receiver<(u64, Reply)>,
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
        std::thread::Builder::new()
            .name("todo-backend".into())
            .spawn(move || {
                let mut ids = HashMap::new();
                // Ends when the app drops its `Backend` (the window closed).
                for job in jobs {
                    if let Some(r) = run(&api, &job.token, job.op, &mut ids) {
                        if reply.send((job.epoch, r)).is_err() {
                            break;
                        }
                        waker();
                    }
                }
            })
            .expect("spawn the backend thread");
        Backend { send, replies }
    }

    /// Queues `op`, tagged with the sign-in it belongs to.
    pub fn run(&self, epoch: u64, token: &str, op: Op) {
        let _ = self.send.send(Job { epoch, token: token.to_string(), op });
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
    })
}
