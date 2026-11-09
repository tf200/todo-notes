use super::*;

fn note(id: &str, title: &str, pinned: bool) -> Note {
    Note {
        id: id.into(),
        title: title.into(),
        body: String::new(),
        pinned,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn run(op: Op) -> Effect {
    Effect::Run(op)
}

fn ids(n: &Notes) -> Vec<&str> {
    n.all().iter().map(|x| x.id.as_str()).collect()
}

/// Loaded: `a` and `b` as the server sent them, and `p`, pinned, sent last.
fn loaded() -> Notes {
    let mut n = Notes::default();
    step(&mut n, Event::Open);
    step(&mut n, Event::Reply(Reply::Notes(Ok(vec![note("a", "A", false), note("b", "B", false), note("p", "P", true)]))));
    n
}

#[test]
fn load_puts_pinned_first_and_opens_the_top_note() {
    let n = loaded();
    assert_eq!(ids(&n), ["p", "a", "b"]);
    assert_eq!(n.open.as_deref(), Some("p"));
}

#[test]
fn a_new_note_goes_under_the_pinned_ones_and_takes_the_server_id() {
    let mut n = loaded();
    let fx = step(&mut n, Event::New);
    assert_eq!(fx, vec![run(Op::CreateNote { tmp: "tmp-note-1".into(), title: "".into(), body: "".into() })]);
    assert_eq!(ids(&n), ["p", "tmp-note-1", "a", "b"]);
    assert_eq!(n.open.as_deref(), Some("tmp-note-1"));
    // Typed before the create came back: kept, and sent (the worker swaps the id).
    let fx = step(&mut n, Event::Edit { id: "tmp-note-1".into(), title: "Ideas".into(), body: "".into() });
    assert_eq!(fx.len(), 1);
    step(&mut n, Event::Reply(Reply::NoteCreated { tmp: "tmp-note-1".into(), result: Ok(note("c", "", false)) }));
    assert_eq!(n.open.as_deref(), Some("c"));
    assert_eq!(n.get("c").unwrap().title, "Ideas");
}

#[test]
fn an_edit_sends_what_changed_and_moves_the_note_up() {
    let mut n = loaded();
    let fx = step(&mut n, Event::Edit { id: "b".into(), title: "B".into(), body: "hello".into() });
    let patch = NotePatch { body: Some("hello".into()), ..Default::default() };
    assert_eq!(fx, vec![run(Op::UpdateNote { id: "b".into(), patch })]);
    assert_eq!(ids(&n), ["p", "b", "a"], "top of the unpinned group, under the pinned note");
    assert!(step(&mut n, Event::Edit { id: "b".into(), title: "B".into(), body: "hello".into() }).is_empty());
}

#[test]
fn pinning_moves_to_the_top() {
    let mut n = loaded();
    step(&mut n, Event::SetPinned { id: "b".into(), pinned: true });
    assert_eq!(ids(&n), ["b", "p", "a"]);
    step(&mut n, Event::SetPinned { id: "b".into(), pinned: false });
    assert_eq!(ids(&n), ["p", "b", "a"]);
}

#[test]
fn deleting_the_open_note_opens_its_neighbour() {
    let mut n = loaded();
    step(&mut n, Event::Select("a".into()));
    let fx = step(&mut n, Event::Delete("a".into()));
    assert_eq!(fx, vec![run(Op::DeleteNote { id: "a".into() })]);
    assert_eq!(n.open.as_deref(), Some("b"));
    step(&mut n, Event::Delete("b".into()));
    assert_eq!(n.open.as_deref(), Some("p"), "the last one falls back to the one above");
}

#[test]
fn a_failed_save_reloads_with_a_notice() {
    let mut n = loaded();
    let fx = step(&mut n, Event::Reply(Reply::NoteSaved(Err(ApiError::Unreachable))));
    assert_eq!(fx, vec![run(Op::LoadNotes)]);
    assert!(n.notice.as_deref().unwrap().starts_with("Couldn't save the note."));
    let fx = step(&mut n, Event::Reply(Reply::NoteDeleted(Err(ApiError::Unauthorized))));
    assert_eq!(fx, vec![Effect::Unauthorized]);
}

#[test]
fn a_reply_keeps_newer_local_text() {
    let mut n = loaded();
    step(&mut n, Event::Edit { id: "a".into(), title: "A".into(), body: "one".into() });
    step(&mut n, Event::Edit { id: "a".into(), title: "A".into(), body: "one two".into() });
    let mut saved = note("a", "A", false);
    saved.body = "one".into();
    saved.updated_at = "2026-11-09T10:00:00Z".into();
    step(&mut n, Event::Reply(Reply::NoteSaved(Ok(saved))));
    let a = n.get("a").unwrap();
    assert_eq!(a.body, "one two");
    assert_eq!(a.updated_at, "2026-11-09T10:00:00Z");
}

#[test]
fn preview_skips_markdown_marks_and_blank_lines() {
    assert_eq!(preview("\n\n# Heading\nbody"), "Heading");
    assert_eq!(preview("- [ ] buy milk"), "buy milk");
    assert_eq!(preview("   "), "No additional text");
    assert_eq!(preview("A **bold** `code` line"), "A bold code line");
}

#[test]
fn toggling_a_task_flips_only_its_marker() {
    let body = "- [ ] one\n- [x] two";
    assert_eq!(toggle_task(body, 2, true).as_deref(), Some("- [x] one\n- [x] two"));
    assert_eq!(toggle_task(body, 12, false).as_deref(), Some("- [ ] one\n- [ ] two"));
    assert_eq!(toggle_task(body, 0, true), None, "not a marker");
    assert_eq!(toggle_task(body, 99, true), None);
}
