//! The todo lists and their tasks: what the screen shows, and the requests that keep the
//! server in step.
//!
//! Edits are optimistic. The model changes at once and queues an [`Op`]; the screen never waits
//! on the server. New items get a temporary id until the server's create returns. When a
//! request fails, the model reloads from the server, so the screen shows what was really
//! saved, and leaves a notice.
//!
//! [`step`] is pure: events in, effects out. `main.rs` runs the effects (`backend.rs`).

use std::collections::HashMap;

use crate::api::{ApiError, Note, NotePatch, Stage, Task, TaskList, TaskPatch};

#[cfg(test)]
mod tests;

/// Something fetched from the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(ApiError),
}

#[derive(Debug, Clone)]
pub struct Todos {
    pub lists: Load<Vec<TaskList>>,
    /// The list the main pane shows.
    pub selected: Option<String>,
    /// Each opened list's tasks, by list id, oldest first.
    pub tasks: HashMap<String, Load<Vec<Task>>>,
    /// The task open in the details pane.
    pub open_task: Option<String>,
    /// The last failure, shown until dismissed.
    pub notice: Option<String>,
    /// Which sign-in this is: replies queued for an earlier one are dropped.
    pub epoch: u64,
    next_temp: u64,
    /// The workflow's steps between to do and done, in order.
    pub stages: Vec<Stage>,
    /// The last move, which can be undone until it's dismissed.
    pub moved: Option<Moved>,
    moves: u64,
}

/// Where a task is in the workflow.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    Todo,
    /// In a step (its id).
    Stage(String),
    Done,
}

/// A task's last move, for "Moved to … · Undo".
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    pub task: String,
    pub from: Place,
    pub to: Place,
    /// Counts moves, so the screen can tell a new one from the one it already shows.
    pub n: u64,
}

impl Default for Todos {
    fn default() -> Self {
        Todos {
            lists: Load::Loading,
            selected: None,
            tasks: HashMap::new(),
            open_task: None,
            notice: None,
            epoch: 0,
            next_temp: 0,
            stages: Vec::new(),
            moved: None,
            moves: 0,
        }
    }
}

/// A request for the backend worker. Ids may be temporary; the worker swaps them.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    LoadLists,
    LoadTasks { list: String },
    CreateList { tmp: String, title: String },
    RenameList { id: String, title: String },
    DeleteList { id: String },
    CreateTask { tmp: String, list: String, title: String },
    UpdateTask { id: String, patch: TaskPatch },
    DeleteTask { id: String },
    LoadStages,
    SaveStages(Vec<Stage>),
    // Notes (`notes.rs`).
    LoadNotes,
    CreateNote { tmp: String, title: String, body: String },
    UpdateNote { id: String, patch: NotePatch },
    DeleteNote { id: String },
}

/// A finished request.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Lists(Result<Vec<TaskList>, ApiError>),
    Tasks { list: String, result: Result<Vec<Task>, ApiError> },
    ListCreated { tmp: String, result: Result<TaskList, ApiError> },
    TaskCreated { tmp: String, list: String, result: Result<Task, ApiError> },
    /// A rename, update or delete.
    Saved(Result<(), ApiError>),
    Stages(Result<Vec<Stage>, ApiError>),
    StagesSaved(Result<Vec<Stage>, ApiError>),
    // Notes (`notes.rs`).
    Notes(Result<Vec<Note>, ApiError>),
    NoteCreated { tmp: String, result: Result<Note, ApiError> },
    NoteSaved(Result<Note, ApiError>),
    NoteDeleted(Result<(), ApiError>),
}

impl Reply {
    /// Whether the notes model handles this reply (else the todo model does).
    pub fn is_note(&self) -> bool {
        matches!(self, Reply::Notes(_) | Reply::NoteCreated { .. } | Reply::NoteSaved(_) | Reply::NoteDeleted(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Signed in: start over and load the lists.
    Open,
    Select(String),
    CreateList(String),
    RenameList { id: String, title: String },
    DeleteList(String),
    /// A task at the end of the selected list.
    AddTask(String),
    SetDone { id: String, done: bool },
    /// Moves a task to a place in the workflow (undoable).
    Move { id: String, to: Place },
    /// Moves a task one place on: to do, each step, done, and from done back to to do.
    Advance(String),
    /// Puts the last moved task back where it was.
    Undo,
    /// Forgets the last move (its undo offer has timed out).
    DismissUndo,
    /// Replaces the workflow's steps. Tasks in a removed step move back a step.
    SaveStages(Vec<Stage>),
    /// Sets the day a task is due (a `due_at` stamp), or removes it.
    SetDue { id: String, due: Option<String> },
    /// Saves the details pane's title and description.
    EditTask { id: String, title: String, description: String },
    DeleteTask(String),
    OpenTask(Option<String>),
    Reload,
    DismissNotice,
    Reply(Reply),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Run(Op),
    /// The token stopped working: back to sign-in.
    Unauthorized,
}

pub fn is_temp(id: &str) -> bool {
    id.starts_with("tmp-")
}

impl Todos {
    pub fn lists(&self) -> &[TaskList] {
        match &self.lists {
            Load::Ready(l) => l,
            _ => &[],
        }
    }

    pub fn selected_list(&self) -> Option<&TaskList> {
        let id = self.selected.as_ref()?;
        self.lists().iter().find(|l| &l.id == id)
    }

    pub fn tasks_of(&self, list: &str) -> Option<&Load<Vec<Task>>> {
        self.tasks.get(list)
    }

    /// Where a task is. A step that isn't in the workflow (any more) reads as to do.
    pub fn place(&self, t: &Task) -> Place {
        match &t.stage {
            _ if t.done => Place::Done,
            Some(id) if self.stages.iter().any(|s| &s.id == id) => Place::Stage(id.clone()),
            _ => Place::Todo,
        }
    }

    /// Every place, in order: to do, the steps, done.
    pub fn places(&self) -> Vec<Place> {
        let mut all = vec![Place::Todo];
        all.extend(self.stages.iter().map(|s| Place::Stage(s.id.clone())));
        all.push(Place::Done);
        all
    }

    /// The place after `p`; after done comes to do again.
    pub fn next_place(&self, p: &Place) -> Place {
        let all = self.places();
        let at = all.iter().position(|x| x == p).unwrap_or(0);
        all.get(at + 1).cloned().unwrap_or(Place::Todo)
    }

    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id == id)
    }

    /// What a place is called: "To do", a step's name, "Done".
    pub fn place_name(&self, p: &Place) -> String {
        match p {
            Place::Todo => "To do".into(),
            Place::Stage(id) => self.stage(id).map_or_else(|| "To do".into(), |s| s.name.clone()),
            Place::Done => "Done".into(),
        }
    }

    /// Moves a task, saving the move; returns where it was, or `None` when it was already there.
    fn put(&mut self, id: &str, to: &Place, fx: &mut Vec<Effect>) -> Option<Place> {
        let task = self.task(id)?;
        let from = self.place(task);
        if &from == to {
            return None;
        }
        let task = self.task_mut(id)?;
        let patch = match to {
            Place::Todo => {
                task.done = false;
                task.stage = None;
                TaskPatch { done: Some(false), stage: Some(String::new()), ..TaskPatch::default() }
            }
            Place::Stage(s) => {
                task.done = false;
                task.stage = Some(s.clone());
                TaskPatch { stage: Some(s.clone()), ..TaskPatch::default() }
            }
            Place::Done => {
                task.done = true;
                task.stage = None;
                TaskPatch { done: Some(true), ..TaskPatch::default() }
            }
        };
        fx.push(Effect::Run(Op::UpdateTask { id: id.to_string(), patch }));
        Some(from)
    }

    /// Moves a task and remembers the move, so it can be undone.
    fn move_task(&mut self, id: String, to: Place, fx: &mut Vec<Effect>) {
        if let Some(from) = self.put(&id, &to, fx) {
            self.moves += 1;
            self.moved = Some(Moved { task: id, from, to, n: self.moves });
        }
    }

    /// Open tasks in a loaded list.
    pub fn open_count(&self, list: &str) -> Option<usize> {
        match self.tasks.get(list) {
            Some(Load::Ready(t)) => Some(t.iter().filter(|t| !t.done).count()),
            _ => None,
        }
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.values().find_map(|l| match l {
            Load::Ready(t) => t.iter().find(|t| t.id == id),
            _ => None,
        })
    }

    fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.values_mut().find_map(|l| match l {
            Load::Ready(t) => t.iter_mut().find(|t| t.id == id),
            _ => None,
        })
    }

    fn temp(&mut self) -> String {
        self.next_temp += 1;
        format!("tmp-{}", self.next_temp)
    }

    /// Shows `list`, loading its tasks the first time (or after a failed load).
    fn select(&mut self, list: String, fx: &mut Vec<Effect>) {
        let needs_load = !matches!(self.tasks.get(&list), Some(Load::Ready(_) | Load::Loading));
        if needs_load {
            self.tasks.insert(list.clone(), Load::Loading);
            fx.push(Effect::Run(Op::LoadTasks { list: list.clone() }));
        }
        if self.selected.as_ref() != Some(&list) {
            self.open_task = None;
        }
        self.selected = Some(list);
    }

    /// After a failed save: say so, and fetch what the server really has.
    fn failed(&mut self, what: &str, e: &ApiError, fx: &mut Vec<Effect>) {
        if *e == ApiError::Unauthorized {
            fx.push(Effect::Unauthorized);
            return;
        }
        self.notice = Some(format!("{what} {}", reason(e)));
        fx.push(Effect::Run(Op::LoadLists));
        if let Some(list) = self.selected.clone().filter(|l| !is_temp(l)) {
            fx.push(Effect::Run(Op::LoadTasks { list }));
        }
    }
}

/// Why a request failed, for a notice.
pub fn reason(e: &ApiError) -> &'static str {
    match e {
        ApiError::Unreachable => "Can't reach the server.",
        ApiError::NotFound => "It was deleted somewhere else.",
        _ => "The server didn't accept it.",
    }
}

pub fn step(t: &mut Todos, event: Event) -> Vec<Effect> {
    let mut fx = Vec::new();
    match event {
        Event::Open => {
            let epoch = t.epoch + 1;
            *t = Todos { epoch, ..Todos::default() };
            // The steps first: the worker runs in order, so they're in before any tasks.
            fx.push(Effect::Run(Op::LoadStages));
            fx.push(Effect::Run(Op::LoadLists));
        }
        Event::Reload => {
            t.notice = None;
            if matches!(t.lists, Load::Failed(_)) {
                t.lists = Load::Loading;
            }
            fx.push(Effect::Run(Op::LoadStages));
            fx.push(Effect::Run(Op::LoadLists));
            if let Some(list) = t.selected.clone().filter(|l| !is_temp(l)) {
                if matches!(t.tasks.get(&list), Some(Load::Failed(_))) {
                    t.tasks.insert(list.clone(), Load::Loading);
                }
                fx.push(Effect::Run(Op::LoadTasks { list }));
            }
        }
        Event::DismissNotice => t.notice = None,
        Event::Select(list) => t.select(list, &mut fx),
        Event::OpenTask(id) => t.open_task = id,

        Event::CreateList(title) => {
            let title = title.trim().to_string();
            if title.is_empty() || !matches!(t.lists, Load::Ready(_)) {
                return fx;
            }
            let tmp = t.temp();
            if let Load::Ready(lists) = &mut t.lists {
                lists.push(TaskList { id: tmp.clone(), title: title.clone(), created_at: String::new() });
            }
            // Nothing to fetch: a new list starts empty.
            t.tasks.insert(tmp.clone(), Load::Ready(Vec::new()));
            t.select(tmp.clone(), &mut fx);
            fx.push(Effect::Run(Op::CreateList { tmp, title }));
        }
        Event::RenameList { id, title } => {
            let title = title.trim().to_string();
            let Load::Ready(lists) = &mut t.lists else { return fx };
            let Some(list) = lists.iter_mut().find(|l| l.id == id) else { return fx };
            if title.is_empty() || title == list.title {
                return fx;
            }
            list.title = title.clone();
            fx.push(Effect::Run(Op::RenameList { id, title }));
        }
        Event::DeleteList(id) => {
            let Load::Ready(lists) = &mut t.lists else { return fx };
            let Some(at) = lists.iter().position(|l| l.id == id) else { return fx };
            lists.remove(at);
            // Select the neighbour that slides into its place, as a source list does.
            let next = lists.get(at).or_else(|| at.checked_sub(1).and_then(|i| lists.get(i))).map(|l| l.id.clone());
            t.tasks.remove(&id);
            if t.selected.as_ref() == Some(&id) {
                t.selected = None;
                t.open_task = None;
                if let Some(next) = next {
                    t.select(next, &mut fx);
                }
            }
            fx.push(Effect::Run(Op::DeleteList { id }));
        }

        Event::AddTask(title) => {
            let title = title.trim().to_string();
            let Some(list) = t.selected.clone() else { return fx };
            if title.is_empty() || !matches!(t.tasks.get(&list), Some(Load::Ready(_))) {
                return fx;
            }
            let tmp = t.temp();
            if let Some(Load::Ready(tasks)) = t.tasks.get_mut(&list) {
                tasks.push(Task {
                    id: tmp.clone(),
                    list_id: list.clone(),
                    title: title.clone(),
                    description: String::new(),
                    due_at: None,
                    done: false,
                    stage: None,
                    created_at: String::new(),
                });
            }
            fx.push(Effect::Run(Op::CreateTask { tmp, list, title }));
        }
        Event::SetDone { id, done } => {
            let Some(task) = t.task(&id) else { return fx };
            if task.done == done {
                return fx;
            }
            t.move_task(id, if done { Place::Done } else { Place::Todo }, &mut fx);
        }
        Event::Move { id, to } => t.move_task(id, to, &mut fx),
        Event::Advance(id) => {
            let Some(task) = t.task(&id) else { return fx };
            let to = t.next_place(&t.place(task));
            t.move_task(id, to, &mut fx);
        }
        Event::Undo => {
            if let Some(m) = t.moved.take() {
                t.put(&m.task, &m.from, &mut fx);
            }
        }
        Event::DismissUndo => t.moved = None,
        Event::SaveStages(stages) => {
            let stages: Vec<Stage> = stages
                .into_iter()
                .map(|s| Stage { name: s.name.trim().to_string(), ..s })
                .filter(|s| !s.name.is_empty())
                .collect();
            if stages == t.stages {
                return fx;
            }
            // As the server does: tasks in a removed step go to the nearest kept step before it.
            let moves = removed(&t.stages, &stages);
            for load in t.tasks.values_mut() {
                if let Load::Ready(tasks) = load {
                    for x in tasks.iter_mut() {
                        if let Some(to) = x.stage.as_ref().and_then(|s| moves.get(s)) {
                            x.stage = to.clone();
                        }
                    }
                }
            }
            let gone = |p: &Place| matches!(p, Place::Stage(s) if moves.contains_key(s));
            if t.moved.as_ref().is_some_and(|m| gone(&m.from) || gone(&m.to)) {
                t.moved = None;
            }
            t.stages = stages.clone();
            fx.push(Effect::Run(Op::SaveStages(stages)));
        }
        Event::SetDue { id, due } => {
            let Some(task) = t.task_mut(&id) else { return fx };
            let day = |d: &Option<String>| d.as_deref().and_then(|d| d.get(..10)).map(str::to_owned);
            if day(&task.due_at) == day(&due) {
                return fx;
            }
            task.due_at = due.clone();
            fx.push(Effect::Run(Op::UpdateTask { id, patch: TaskPatch { due_at: Some(due), ..TaskPatch::default() } }));
        }
        Event::EditTask { id, title, description } => {
            let title = title.trim().to_string();
            let Some(task) = t.task_mut(&id) else { return fx };
            let mut patch = TaskPatch::default();
            if !title.is_empty() && title != task.title {
                task.title = title.clone();
                patch.title = Some(title);
            }
            if description != task.description {
                task.description = description.clone();
                patch.description = Some(description);
            }
            if patch != TaskPatch::default() {
                fx.push(Effect::Run(Op::UpdateTask { id, patch }));
            }
        }
        Event::DeleteTask(id) => {
            let mut found = false;
            for load in t.tasks.values_mut() {
                if let Load::Ready(tasks) = load {
                    let before = tasks.len();
                    tasks.retain(|x| x.id != id);
                    found |= tasks.len() != before;
                }
            }
            if !found {
                return fx;
            }
            if t.open_task.as_ref() == Some(&id) {
                t.open_task = None;
            }
            if t.moved.as_ref().is_some_and(|m| m.task == id) {
                t.moved = None;
            }
            fx.push(Effect::Run(Op::DeleteTask { id }));
        }

        Event::Reply(reply) => reply_step(t, reply, &mut fx),
    }
    fx
}

fn reply_step(t: &mut Todos, reply: Reply, fx: &mut Vec<Effect>) {
    match reply {
        Reply::Lists(Ok(mut lists)) => {
            // Lists still being created stay, after the server's.
            for l in t.lists().iter().filter(|l| is_temp(&l.id)) {
                lists.push(l.clone());
            }
            t.tasks.retain(|id, _| lists.iter().any(|l| &l.id == id));
            let keep = t.selected.clone().filter(|id| lists.iter().any(|l| &l.id == id));
            let first = lists.first().map(|l| l.id.clone());
            t.lists = Load::Ready(lists);
            match keep.or(first) {
                Some(list) => t.select(list, fx),
                None => {
                    t.selected = None;
                    t.open_task = None;
                }
            }
        }
        Reply::Lists(Err(ApiError::Unauthorized)) => fx.push(Effect::Unauthorized),
        Reply::Lists(Err(e)) => {
            // Keep lists already on screen; a first load shows the failure.
            if !matches!(t.lists, Load::Ready(_)) {
                t.lists = Load::Failed(e);
            } else {
                t.notice = Some(format!("Couldn't refresh. {}", reason(&e)));
            }
        }
        Reply::Tasks { list, result } => {
            if !t.tasks.contains_key(&list) {
                return; // The list was deleted meanwhile.
            }
            match result {
                Ok(mut tasks) => {
                    // Tasks still being created stay, after the server's.
                    if let Some(Load::Ready(old)) = t.tasks.get(&list) {
                        tasks.extend(old.iter().filter(|x| is_temp(&x.id)).cloned());
                    }
                    if t.open_task.as_ref().is_some_and(|o| !tasks.iter().any(|x| &x.id == o)) {
                        t.open_task = None;
                    }
                    t.tasks.insert(list, Load::Ready(tasks));
                }
                Err(ApiError::Unauthorized) => fx.push(Effect::Unauthorized),
                Err(e) => {
                    if !matches!(t.tasks.get(&list), Some(Load::Ready(_))) {
                        t.tasks.insert(list, Load::Failed(e));
                    }
                }
            }
        }
        Reply::ListCreated { tmp, result: Ok(list) } => {
            let real = list.id.clone();
            if let Load::Ready(lists) = &mut t.lists {
                if let Some(l) = lists.iter_mut().find(|l| l.id == tmp) {
                    // Keep a rename made while the create was on its way.
                    *l = TaskList { title: l.title.clone(), ..list };
                }
            }
            if let Some(mut tasks) = t.tasks.remove(&tmp) {
                if let Load::Ready(ts) = &mut tasks {
                    for x in ts.iter_mut() {
                        x.list_id = real.clone();
                    }
                }
                t.tasks.insert(real.clone(), tasks);
            }
            if t.selected.as_ref() == Some(&tmp) {
                t.selected = Some(real);
            }
        }
        Reply::ListCreated { tmp, result: Err(e) } => {
            if let Load::Ready(lists) = &mut t.lists {
                lists.retain(|l| l.id != tmp);
            }
            t.tasks.remove(&tmp);
            if t.selected.as_ref() == Some(&tmp) {
                t.selected = t.lists().first().map(|l| l.id.clone());
                t.open_task = None;
            }
            t.failed("Couldn't create the list.", &e, fx);
        }
        Reply::TaskCreated { tmp, result: Ok(task), .. } => {
            // Keep edits made while the create was on its way: the worker sends them after it.
            if let Some(x) = t.task_mut(&tmp) {
                x.id = task.id.clone();
                x.created_at = task.created_at;
            }
            if t.open_task.as_ref() == Some(&tmp) {
                t.open_task = Some(task.id);
            }
        }
        Reply::TaskCreated { tmp, result: Err(e), .. } => {
            for load in t.tasks.values_mut() {
                if let Load::Ready(tasks) = load {
                    tasks.retain(|x| x.id != tmp);
                }
            }
            if t.open_task.as_ref() == Some(&tmp) {
                t.open_task = None;
            }
            t.failed("Couldn't add the task.", &e, fx);
        }
        Reply::Saved(Ok(())) => {}
        Reply::Stages(Ok(stages)) => t.stages = stages,
        Reply::Stages(Err(ApiError::Unauthorized)) | Reply::StagesSaved(Err(ApiError::Unauthorized)) => {
            fx.push(Effect::Unauthorized)
        }
        // A server without workflows (404) or a failed load: no steps, tasks are to do or done.
        Reply::Stages(Err(_)) => {}
        Reply::StagesSaved(Ok(stages)) => t.stages = stages,
        Reply::StagesSaved(Err(e)) => {
            t.failed("Couldn't save the workflow.", &e, fx);
            fx.push(Effect::Run(Op::LoadStages));
        }
        Reply::Saved(Err(e)) => t.failed("Couldn't save your change.", &e, fx),
        // `notes::step` handles these; `main.rs` routes them there.
        Reply::Notes(_) | Reply::NoteCreated { .. } | Reply::NoteSaved(_) | Reply::NoteDeleted(_) => {}
    }
}

/// Where the tasks of each step in `old` that isn't in `next` go: the nearest kept step before
/// it, or `None` (to do). The server does the same with its copy.
pub fn removed(old: &[Stage], next: &[Stage]) -> HashMap<String, Option<String>> {
    let mut moves = HashMap::new();
    let mut last = None;
    for s in old {
        if next.iter().any(|n| n.id == s.id) {
            last = Some(s.id.clone());
        } else {
            moves.insert(s.id.clone(), last.clone());
        }
    }
    moves
}

/// A new step's id: unique enough for one user's handful of steps.
pub fn new_stage_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    format!("s{nanos:x}{:x}", N.fetch_add(1, Ordering::Relaxed))
}
