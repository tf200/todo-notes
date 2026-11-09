use super::*;

fn list(id: &str, title: &str) -> TaskList {
    TaskList { id: id.into(), title: title.into(), created_at: String::new() }
}

fn task(id: &str, list: &str, title: &str, done: bool) -> Task {
    Task {
        id: id.into(),
        list_id: list.into(),
        title: title.into(),
        description: String::new(),
        due_at: None,
        done,
        created_at: String::new(),
    }
}

fn run(op: Op) -> Effect {
    Effect::Run(op)
}

/// Signed in, with lists `a` (selected, tasks loaded) and `b`.
fn loaded() -> Todos {
    let mut t = Todos::default();
    step(&mut t, Event::Open);
    step(&mut t, Event::Reply(Reply::Lists(Ok(vec![list("a", "Home"), list("b", "Work")]))));
    step(
        &mut t,
        Event::Reply(Reply::Tasks {
            list: "a".into(),
            result: Ok(vec![task("t1", "a", "Milk", false), task("t2", "a", "Eggs", true)]),
        }),
    );
    t
}

fn titles(t: &Todos, list: &str) -> Vec<String> {
    match t.tasks_of(list) {
        Some(Load::Ready(ts)) => ts.iter().map(|x| x.title.clone()).collect(),
        other => panic!("tasks of {list} not ready: {other:?}"),
    }
}

#[test]
fn open_loads_lists_then_the_first_lists_tasks() {
    let mut t = Todos::default();
    assert_eq!(step(&mut t, Event::Open), vec![run(Op::LoadLists)]);
    assert_eq!(t.lists, Load::Loading);
    let fx = step(&mut t, Event::Reply(Reply::Lists(Ok(vec![list("a", "Home"), list("b", "Work")]))));
    assert_eq!(fx, vec![run(Op::LoadTasks { list: "a".into() })]);
    assert_eq!(t.selected.as_deref(), Some("a"));
    assert_eq!(t.tasks_of("a"), Some(&Load::Loading));
}

#[test]
fn no_lists_selects_nothing() {
    let mut t = Todos::default();
    step(&mut t, Event::Open);
    assert!(step(&mut t, Event::Reply(Reply::Lists(Ok(vec![])))).is_empty());
    assert_eq!(t.selected, None);
    assert_eq!(t.lists, Load::Ready(vec![]));
}

#[test]
fn selecting_loads_once() {
    let mut t = loaded();
    assert_eq!(step(&mut t, Event::Select("b".into())), vec![run(Op::LoadTasks { list: "b".into() })]);
    step(&mut t, Event::Reply(Reply::Tasks { list: "b".into(), result: Ok(vec![]) }));
    step(&mut t, Event::Select("a".into()));
    assert!(step(&mut t, Event::Select("b".into())).is_empty(), "cached, no reload");
}

#[test]
fn a_401_anywhere_signs_out() {
    let mut t = loaded();
    let fx = step(&mut t, Event::Reply(Reply::Lists(Err(ApiError::Unauthorized))));
    assert_eq!(fx, vec![Effect::Unauthorized]);
    let fx = step(&mut t, Event::Reply(Reply::Saved(Err(ApiError::Unauthorized))));
    assert_eq!(fx, vec![Effect::Unauthorized]);
}

#[test]
fn first_load_failure_shows_and_reload_retries() {
    let mut t = Todos::default();
    step(&mut t, Event::Open);
    step(&mut t, Event::Reply(Reply::Lists(Err(ApiError::Unreachable))));
    assert_eq!(t.lists, Load::Failed(ApiError::Unreachable));
    assert_eq!(step(&mut t, Event::Reload), vec![run(Op::LoadLists)]);
    assert_eq!(t.lists, Load::Loading);
}

#[test]
fn adding_a_task_shows_it_at_once_then_takes_the_server_id() {
    let mut t = loaded();
    let fx = step(&mut t, Event::AddTask("  Bread ".into()));
    assert_eq!(fx, vec![run(Op::CreateTask { tmp: "tmp-1".into(), list: "a".into(), title: "Bread".into() })]);
    assert_eq!(titles(&t, "a"), ["Milk", "Eggs", "Bread"]);
    // Ticked before the server answered: the tick survives the reply.
    step(&mut t, Event::SetDone { id: "tmp-1".into(), done: true });
    step(
        &mut t,
        Event::Reply(Reply::TaskCreated {
            tmp: "tmp-1".into(),
            list: "a".into(),
            result: Ok(task("t3", "a", "Bread", false)),
        }),
    );
    let bread = t.task("t3").expect("renamed to the server id");
    assert!(bread.done);
    assert!(t.task("tmp-1").is_none());
}

#[test]
fn blank_titles_are_ignored() {
    let mut t = loaded();
    assert!(step(&mut t, Event::AddTask("   ".into())).is_empty());
    assert!(step(&mut t, Event::CreateList("".into())).is_empty());
    assert!(step(&mut t, Event::RenameList { id: "a".into(), title: " ".into() }).is_empty());
}

#[test]
fn a_failed_create_takes_the_task_back_and_reloads() {
    let mut t = loaded();
    step(&mut t, Event::AddTask("Bread".into()));
    let fx = step(
        &mut t,
        Event::Reply(Reply::TaskCreated { tmp: "tmp-1".into(), list: "a".into(), result: Err(ApiError::Unreachable) }),
    );
    assert_eq!(titles(&t, "a"), ["Milk", "Eggs"]);
    assert_eq!(fx, vec![run(Op::LoadLists), run(Op::LoadTasks { list: "a".into() })]);
    assert!(t.notice.as_deref().unwrap().contains("Couldn't add the task."));
}

#[test]
fn toggling_sends_only_done() {
    let mut t = loaded();
    let fx = step(&mut t, Event::SetDone { id: "t1".into(), done: true });
    assert_eq!(
        fx,
        vec![run(Op::UpdateTask { id: "t1".into(), patch: TaskPatch { done: Some(true), ..Default::default() } })]
    );
    assert_eq!(t.open_count("a"), Some(0));
    assert!(step(&mut t, Event::SetDone { id: "t1".into(), done: true }).is_empty(), "no change, no request");
}

#[test]
fn editing_sends_only_what_changed() {
    let mut t = loaded();
    let fx = step(&mut t, Event::EditTask { id: "t1".into(), title: "Milk".into(), description: "2 litres".into() });
    let patch = TaskPatch { description: Some("2 litres".into()), ..Default::default() };
    assert_eq!(fx, vec![run(Op::UpdateTask { id: "t1".into(), patch })]);
    assert!(step(&mut t, Event::EditTask { id: "t1".into(), title: "Milk".into(), description: "2 litres".into() })
        .is_empty());
}

#[test]
fn deleting_the_selected_list_selects_its_neighbour() {
    let mut t = loaded();
    let fx = step(&mut t, Event::DeleteList("a".into()));
    assert_eq!(t.selected.as_deref(), Some("b"));
    assert_eq!(fx, vec![run(Op::LoadTasks { list: "b".into() }), run(Op::DeleteList { id: "a".into() })]);
    assert!(t.tasks_of("a").is_none());
}

#[test]
fn a_new_list_is_selected_and_takes_the_server_id() {
    let mut t = loaded();
    let fx = step(&mut t, Event::CreateList("Groceries".into()));
    assert_eq!(fx, vec![run(Op::CreateList { tmp: "tmp-1".into(), title: "Groceries".into() })]);
    assert_eq!(t.selected.as_deref(), Some("tmp-1"));
    step(&mut t, Event::AddTask("Apples".into()));
    step(
        &mut t,
        Event::Reply(Reply::ListCreated { tmp: "tmp-1".into(), result: Ok(list("c", "Groceries")) }),
    );
    assert_eq!(t.selected.as_deref(), Some("c"));
    assert_eq!(titles(&t, "c"), ["Apples"]);
    assert_eq!(t.task("tmp-2").unwrap().list_id, "c");
}

#[test]
fn a_reload_keeps_tasks_still_being_created() {
    let mut t = loaded();
    step(&mut t, Event::AddTask("Bread".into()));
    step(
        &mut t,
        Event::Reply(Reply::Tasks { list: "a".into(), result: Ok(vec![task("t1", "a", "Milk", false)]) }),
    );
    assert_eq!(titles(&t, "a"), ["Milk", "Bread"]);
}

#[test]
fn a_new_sign_in_starts_over() {
    let mut t = loaded();
    let epoch = t.epoch;
    step(&mut t, Event::Open);
    assert_eq!(t.epoch, epoch + 1);
    assert_eq!(t.lists, Load::Loading);
    assert!(t.tasks.is_empty());
}
