//! The todo screen: lists in the sidebar, the selected list's tasks in the middle, and the open
//! task's details on the right. rune-kit's shell components on the OpenCode theme.

use rune::{Cx, El, Icon, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::shell::{self, SidebarToggle};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, Dialog, IconButton, IconButtonVariant, Kit, MenuItem, Placement,
    Popover, SegmentedControl, Sidebar, SidebarRow, SidebarSection, Spinner, SplitView, TextArea, TextInput, Toolbar,
    context_menu,
};

use crate::api::{Task, TaskList};
use crate::dates;
use crate::session::Session;
use crate::todos::{Event, Load};
use crate::{App, MenuTarget, NEW_LIST, Section};

pub(crate) static PLUS: Icon = Icon {
    name: "todo-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};
static LIST: Icon = Icon {
    name: "todo-list",
    view: 16.0,
    body: r#"<circle cx="3.75" cy="4.5" r="0.9" fill="currentColor"/><circle cx="3.75" cy="8" r="0.9" fill="currentColor"/><circle cx="3.75" cy="11.5" r="0.9" fill="currentColor"/><path d="M6.5 4.5H12.75M6.5 8H12.75M6.5 11.5H12.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
pub(crate) static NOTE: Icon = Icon {
    name: "todo-note",
    view: 16.0,
    body: r#"<path d="M4 3H12V13H4Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/><path d="M6 6H10M6 8.5H10M6 11H8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static CALENDAR: Icon = Icon {
    name: "todo-calendar",
    view: 16.0,
    body: r#"<rect x="2.75" y="3.5" width="10.5" height="9.75" rx="1.75" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M2.75 6.75H13.25M5.5 2.25V4.5M10.5 2.25V4.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static PREV: Icon = Icon {
    name: "todo-prev",
    view: 16.0,
    body: r#"<path d="M9.75 4L5.75 8L9.75 12" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static NEXT: Icon = Icon {
    name: "todo-next",
    view: 16.0,
    body: r#"<path d="M6.25 4L10.25 8L6.25 12" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The signed-in screen and its overlays (menus, the delete confirmation).
pub fn view(s: &App, session: &Session, k: &Kit, cx: &mut Cx<App>) -> El {
    let mut layer = Vec::new();
    let split = match s.section {
        Section::Tasks => tasks_split(s, session, k, cx, &mut layer),
        Section::Notes => crate::notes_ui::split(s, session, k, cx, &mut layer),
    };
    confirm_delete(s, k, &mut layer);
    rune_kit::root(split, layer)
}

/// The sidebar's frame, the same in both sections: the brand, the Tasks/Notes switch and the
/// account row. Each section adds its "New" row and its rows.
pub(crate) fn sidebar_frame(s: &App, session: &Session, k: &Kit) -> Sidebar {
    let pick = SegmentedControl::new(&["Tasks", "Notes"], Some(usize::from(s.section == Section::Notes)))
        .label("Section")
        .width(None)
        .on_pick(|s: &mut App, i, _: &mut Tx| s.set_section(if i == 0 { Section::Tasks } else { Section::Notes }))
        .build(k)
        .key("section");
    Sidebar::new()
        .header_padding(4.0, 8.0)
        .header(brand(k))
        .header(row().key("section-row").w_full().px(4.0).pb(8.0).child(pick))
        .footer(account(session, k))
}

fn tasks_split(s: &App, session: &Session, k: &Kit, cx: &mut Cx<App>, layer: &mut Vec<El>) -> El {
    let title = s.todos.selected_list().map_or("Todo & Notes", |l| l.title.as_str());
    let toolbar = Toolbar::new().title(title).trailing(
        IconButton::new(&shell::INSPECTOR)
            .variant(IconButtonVariant::GhostMuted)
            .expanded(s.details.is_open())
            .label(if s.details.is_open() { "Hide details" } else { "Show details" })
            .on_press(|s: &mut App| {
                if s.details.is_open() { s.close_task() } else { s.details.set_open(true) }
            })
            .build(k)
            .key("details-toggle"),
    );
    SplitView::new(main_pane(s, k, layer))
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar(s, session, k, layer))
        .inspector(&s.details, |s: &mut App| &mut s.details, details(s, k, layer))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(10.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().in_header())
        .reduced_motion(cx.is_reduced_motion())
        .build(k)
}

// The sidebar.

fn sidebar(s: &App, session: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let new_list = SidebarRow::new("New list")
        .icon(&PLUS)
        .detail("Ctrl+Shift+N")
        .on_select(|s: &mut App, tx: &mut Tx| s.start_new_list(tx))
        .build(k)
        .key("new-list");
    let sb = sidebar_frame(s, session, k).label("Lists").header(new_list);
    let sb = match &s.todos.lists {
        Load::Loading => sb.child(row().key("lists-loading").w_full().py(16.0).center().child(Spinner::new().build(k))),
        Load::Failed(e) => sb.child(
            col()
                .key("lists-failed")
                .w_full()
                .padding(12.0)
                .gap(8.0)
                .child(text(e.message()).font(k.ty.small).color(k.c.text_muted).wrap())
                .child(retry(k)),
        ),
        Load::Ready(lists) => {
            let mut rows: Vec<El> = lists.iter().map(|l| list_row(s, l, k, layer)).collect();
            if s.renaming.as_deref() == Some(NEW_LIST) {
                rows.push(renaming(s, SidebarRow::new("New list").icon(&LIST)).build(k).key(NEW_LIST));
            }
            sb.section(SidebarSection::new("Lists").count(lists.len()).rows(rows))
        }
    };
    sb.build(k)
}

fn brand(k: &Kit) -> El {
    row()
        .key("brand")
        .w_full()
        .px(8.0)
        .pb(8.0)
        .gap(8.0)
        .center_y()
        .child(crate::ui::logo(20.0))
        .child(text("Todo & Notes").font(k.ty.ui_medium).color(k.c.text_base))
}

fn account(session: &Session, k: &Kit) -> El {
    let initial = session.email.chars().next().unwrap_or('?').to_ascii_uppercase().to_string();
    row()
        .key("account")
        .w_full()
        .px(8.0)
        .py(6.0)
        .gap(8.0)
        .center_y()
        .child(
            row()
                .size(22.0, 22.0)
                .radius(11.0)
                .center()
                .linear_gradient(std::f32::consts::PI * 0.75, [(0.0, crate::ui::LOGO_FROM), (1.0, crate::ui::LOGO_TO)])
                .child(text(initial).font(k.ty.small_medium).color(rune::Color::WHITE)),
        )
        .child(text(session.email.clone()).font(k.ty.small).color(k.c.text_muted).grow(1.0).shrink().truncate())
        .child(
            Button::new("Sign out")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.sign_out(tx))
                .build(k)
                .key("sign-out"),
        )
}

/// Puts the row's label into an edit field bound to `App::rename`.
fn renaming(s: &App, r: SidebarRow) -> SidebarRow {
    r.renaming(
        &s.rename,
        |s: &mut App| &mut s.rename,
        |s: &mut App, _: &mut Tx| s.commit_rename(),
        |s: &mut App, _: &mut Tx| s.renaming = None,
    )
}

fn list_row(s: &App, l: &TaskList, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = l.id.clone();
    let mut r = SidebarRow::new(l.title.clone())
        .icon(&LIST)
        .selected(s.todos.selected.as_ref() == Some(&l.id))
        .on_select({
            let id = id.clone();
            move |s: &mut App, _: &mut Tx| s.select_list(id.clone())
        })
        .on_double_click({
            let id = id.clone();
            move |s: &mut App, tx: &mut Tx| s.start_rename(id.clone(), tx)
        });
    if let Some(n) = s.todos.open_count(&l.id).filter(|n| *n > 0) {
        r = r.detail(n.to_string());
    }
    if s.renaming.as_deref() == Some(l.id.as_str()) {
        r = renaming(s, r);
    }
    let built = r.build(k).key(l.id.clone()).on_secondary_press({
        let id = id.clone();
        move |s: &mut App, p, _: &mut Tx| s.menu = Some((MenuTarget::List(id.clone()), p))
    });
    let point = match &s.menu {
        Some((MenuTarget::List(m), p)) if *m == l.id => Some(*p),
        _ => None,
    };
    context_menu(format!("list-menu-{id}"), built, point)
        .item(MenuItem::new("Rename").on_select_tx({
            let id = id.clone();
            move |s: &mut App, tx: &mut Tx| s.start_rename(id.clone(), tx)
        }))
        .item(MenuItem::new("Delete list…").danger(true).on_select(move |s: &mut App| {
            s.menu = None;
            s.confirm_delete = Some(id.clone());
        }))
        .on_close(|s: &mut App| s.menu = None)
        .build(k, layer)
        .key(l.id.clone())
}

// The main pane.

fn main_pane(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let body = match (&s.todos.lists, s.todos.selected_list()) {
        (Load::Loading, _) => centered(Spinner::new().build(k)),
        (Load::Failed(e), _) => empty_state("Couldn't load your lists", e.message(), Some(retry(k)), k),
        (Load::Ready(_), Some(list)) => tasks_view(s, list, k, layer),
        (Load::Ready(_), None) => empty_state(
            "No lists yet",
            "Create a list to start adding tasks.",
            Some(
                Button::new("New list")
                    .variant(ButtonVariant::Submit)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.start_new_list(tx))
                    .build(k)
                    .key("first-list"),
            ),
            k,
        ),
    };
    let mut pane = col().key("main").w_full().h_full().min_h(0.0);
    if let Some(n) = &s.todos.notice {
        pane = pane.child(notice_bar(n, |s: &mut App| s.todo(Event::DismissNotice), k));
    }
    pane.child(body)
}

pub(crate) fn centered(e: El) -> El {
    col().key("centered").w_full().grow(1.0).center().child(e)
}

pub(crate) fn empty_state(title: &str, detail: &str, action: Option<El>, k: &Kit) -> El {
    let mut c = col()
        .key("empty")
        .gap(8.0)
        .items_center()
        .child(text(title.to_string()).font(TextStyle::new(16.0, 600.0)).color(k.c.text_strong))
        .child(text(detail.to_string()).font(k.ty.ui).color(k.c.text_muted));
    if let Some(a) = action {
        c = c.child(a.mt(8.0));
    }
    centered(c)
}

fn retry(k: &Kit) -> El {
    Button::new("Try again").on_press(|s: &mut App| s.todo(Event::Reload)).build(k).key("retry")
}

/// A failure message across the top of the main pane.
pub(crate) fn notice_bar(message: &str, dismiss: impl Fn(&mut App) + 'static, k: &Kit) -> El {
    row()
        .key("notice")
        .w_full()
        .px(16.0)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .fill(k.c.danger.fg.alpha(0.08))
        .child(text(message.to_string()).font(k.ty.small).color(k.c.danger.fg).grow(1.0).shrink().wrap())
        .child(
            Button::new("Dismiss")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(dismiss)
                .build(k)
                .key("dismiss"),
        )
}

fn tasks_view(s: &App, list: &TaskList, k: &Kit, layer: &mut Vec<El>) -> El {
    let tasks = s.todos.tasks_of(&list.id);
    let (open, done): (Vec<&Task>, Vec<&Task>) = match tasks {
        Some(Load::Ready(ts)) => ts.iter().partition(|t| !t.done),
        _ => (Vec::new(), Vec::new()),
    };
    let summary = match tasks {
        Some(Load::Ready(_)) => match (open.len(), done.len()) {
            (0, 0) => "No tasks yet".to_string(),
            (o, 0) => format!("{o} to do"),
            (0, d) => format!("All {d} done"),
            (o, d) => format!("{o} to do · {d} done"),
        },
        _ => String::new(),
    };
    let header = col()
        .key("list-header")
        .w_full()
        .gap(4.0)
        .child(text(list.title.clone()).font(TextStyle::new(24.0, 600.0)).color(k.c.text_strong).truncate())
        .child(text(summary).font(k.ty.ui).color(k.c.text_muted));
    let ready = matches!(tasks, Some(Load::Ready(_)));
    let add = TextInput::new(&s.new_task)
        .placeholder("Add a task, then press Enter")
        .leading_icon(&PLUS)
        .width(None)
        .large()
        .disabled(!ready)
        .on_edit(|s: &mut App| &mut s.new_task)
        .on_submit(|s: &mut App, _: &mut Tx| s.add_task())
        .build(k)
        .key("new-task");

    let mut content = col().key("tasks").w_full().max_w(720.0).gap(16.0).child(header).child(add);
    content = match tasks {
        None | Some(Load::Loading) => content.child(row().key("tasks-loading").w_full().py(24.0).center().child(Spinner::new().build(k))),
        Some(Load::Failed(e)) => content.child(
            col()
                .key("tasks-failed")
                .gap(8.0)
                .child(text(e.message()).font(k.ty.ui).color(k.c.text_muted))
                .child(retry(k)),
        ),
        Some(Load::Ready(_)) => {
            let mut c = content;
            if open.is_empty() && done.is_empty() {
                c = c.child(
                    text("Nothing here yet. Add your first task above.")
                        .key("no-tasks")
                        .font(k.ty.ui)
                        .color(k.c.text_faint)
                        .mt(8.0),
                );
            }
            if !open.is_empty() {
                c = c.child(col().key("open").w_full().gap(2.0).children(open.iter().map(|t| task_row(s, t, k, layer))));
            }
            if !done.is_empty() {
                let label = format!("{} Completed · {}", if s.show_done { "▾" } else { "▸" }, done.len());
                let mut section = col().key("done").w_full().gap(2.0).child(
                    row().key("done-header").w_full().mb(4.0).child(
                        Button::new(label)
                            .variant(ButtonVariant::GhostMuted)
                            .control_size(ControlSize::Small)
                            .on_press(|s: &mut App| s.show_done = !s.show_done)
                            .build(k)
                            .key("done-toggle"),
                    ),
                );
                if s.show_done {
                    section = section.children(done.iter().map(|t| task_row(s, t, k, layer)));
                }
                c = c.child(section)
            }
            c
        }
    };
    col()
        .key("task-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().key("task-page").w_full().px(32.0).pt(28.0).pb(48.0).items_center().child(content))
}

fn task_row(s: &App, t: &Task, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = t.id.clone();
    let done = t.done;
    let check = Checkbox::new(done)
        .label(t.title.clone())
        .hide_label()
        .on_toggle({
            let id = id.clone();
            move |s: &mut App| s.todo(Event::SetDone { id: id.clone(), done: !done })
        })
        .build(k)
        .key("check");
    let mut title = text(t.title.clone())
        .key("title")
        .font(k.ty.ui)
        .color(if done { k.c.text_faint } else { k.c.text_base })
        .grow(1.0)
        .shrink()
        .truncate();
    if done {
        title = title.strike(0, t.title.len(), None);
    }
    let open = s.todos.open_task.as_ref() == Some(&t.id);
    let mut r = row()
        .key(t.id.clone())
        .w_full()
        .px(10.0)
        .py(9.0)
        // The checkbox keeps 12 points after its box for its (hidden) label.
        .gap(0.0)
        .center_y()
        .radius(k.radius.md)
        .child(check)
        .child(title);
    if !t.description.trim().is_empty() {
        r = r.child(icon(&NOTE, 14.0).key("has-notes").color(k.c.text_faint).ml(12.0));
    }
    if let Some(day) = t.due_at.as_deref().and_then(dates::parse) {
        // Overdue reads in red until it's done; due today in the accent colour.
        let color = match day.cmp(&s.today) {
            _ if done => k.c.text_faint,
            std::cmp::Ordering::Less => k.c.danger.fg,
            std::cmp::Ordering::Equal => k.c.text_accent,
            std::cmp::Ordering::Greater => k.c.text_muted,
        };
        r = r.child(
            row()
                .key("due")
                .gap(4.0)
                .center_y()
                .ml(12.0)
                .child(icon(&CALENDAR, 13.0).color(color))
                .child(text(dates::label(day, s.today)).font(k.ty.small).color(color)),
        );
    }
    // A light wash, so the checkbox's outline still shows on the open row.
    let wash = k.c.text_base.alpha(if k.dark { 0.06 } else { 0.05 });
    r = if open { r.fill(wash) } else { r.hover(wash) };
    let r = r
        .on_press({
            let id = id.clone();
            move |s: &mut App| s.open_task(id.clone())
        })
        .on_secondary_press({
            let id = id.clone();
            move |s: &mut App, p, _: &mut Tx| s.menu = Some((MenuTarget::Task(id.clone()), p))
        });
    let point = match &s.menu {
        Some((MenuTarget::Task(m), p)) if *m == t.id => Some(*p),
        _ => None,
    };
    let has_due = t.due_at.is_some();
    let menu = context_menu(format!("task-menu-{id}"), r, point)
        .item(MenuItem::new("Open details").on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.open_task(id.clone());
            }
        }))
        .item(MenuItem::new(if done { "Mark as not done" } else { "Mark as done" }).on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.todo(Event::SetDone { id: id.clone(), done: !done });
            }
        }))
        .separator()
        .item(MenuItem::new("Due today").icon(&CALENDAR).on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.set_due(id.clone(), Some(s.today));
            }
        }))
        .item(MenuItem::new("Due tomorrow").on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.set_due(id.clone(), Some(dates::add_days(s.today, 1)));
            }
        }))
        .item(MenuItem::new("Due next week").on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.set_due(id.clone(), Some(dates::next_week(s.today)));
            }
        }));
    let menu = if has_due {
        menu.item(MenuItem::new("Remove due date").on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.set_due(id.clone(), None);
            }
        }))
    } else {
        menu
    };
    menu.separator()
        .item(MenuItem::new("Delete task").danger(true).on_select(move |s: &mut App| {
            s.menu = None;
            s.todo(Event::DeleteTask(id.clone()));
        }))
        .on_close(|s: &mut App| s.menu = None)
        .build(k, layer)
        .key(t.id.clone())
}

// The details pane.

fn details(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let pane = col().key("details").w_full().padding(16.0).gap(16.0);
    let Some(task) = s.todos.open_task.as_ref().and_then(|id| s.todos.task(id)) else {
        return pane.child(
            text("Select a task to see its details.").font(k.ty.ui).color(k.c.text_muted).wrap(),
        );
    };
    let id = task.id.clone();
    let done = task.done;
    let label = |t: &str| text(t.to_string()).font(k.ty.small_medium).color(k.c.text_muted);
    let title = TextInput::new(&s.task_title)
        .width(None)
        .on_edit(|s: &mut App| &mut s.task_title)
        .on_change(|s: &mut App, tx: &mut Tx| s.edited_task(tx))
        .on_submit(|s: &mut App, _: &mut Tx| s.save_open_task())
        .build(k)
        .key("task-title");
    let notes = TextArea::new(&s.task_notes)
        .placeholder("Add notes")
        .width(None)
        .auto_grow(5, 16)
        .on_edit(|s: &mut App| &mut s.task_notes)
        .on_change(|s: &mut App, tx: &mut Tx| s.edited_task(tx))
        .build(k)
        .key("task-notes");
    let done_box = Checkbox::new(done)
        .label("Done")
        .on_toggle({
            let id = id.clone();
            move |s: &mut App| s.todo(Event::SetDone { id: id.clone(), done: !done })
        })
        .build(k)
        .key("task-done");
    let actions = row()
        .key("task-actions")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(
            // Edits save by themselves a moment after typing stops.
            text(if s.task_edited() { "Editing…" } else { "Saved" })
                .key("task-saved")
                .font(k.ty.small)
                .color(k.c.text_faint),
        )
        .child(el().grow(1.0))
        .child(
            Button::new("Delete")
                .variant(ButtonVariant::Danger)
                .on_press(move |s: &mut App| {
                    s.details.set_open(false);
                    s.todo(Event::DeleteTask(id.clone()));
                })
                .build(k)
                .key("task-delete"),
        );
    let mut pane = pane
        .child(col().gap(6.0).child(label("Title")).child(title))
        .child(col().gap(6.0).child(label("Due")).child(due_picker(s, task, k, layer)))
        .child(col().gap(6.0).child(label("Notes")).child(notes))
        .child(done_box)
        .child(actions);
    if let Some(day) = task.created_at.get(..10) {
        pane = pane.child(text(format!("Created {day}")).font(k.ty.small).color(k.c.text_faint));
    }
    pane
}

/// The due-date button, and the picker it opens: quick days, a month, and "Remove".
fn due_picker(s: &App, task: &Task, k: &Kit, layer: &mut Vec<El>) -> El {
    let due = task.due_at.as_deref().and_then(dates::parse);
    let trigger = Button::new(due.map_or_else(|| "Add a due date".to_string(), |d| dates::long_label(d, s.today)))
        .variant(if due.is_some() { ButtonVariant::Neutral } else { ButtonVariant::GhostMuted })
        .icon(&CALENDAR)
        .expanded(s.due_picker.is_some())
        .on_press(|s: &mut App| s.toggle_due_picker())
        .build(k)
        .key("due-button");
    let mut pop = Popover::new("due-picker")
        .trigger(trigger)
        .label("Due date")
        .placement(Placement::BottomStart)
        .open(s.due_picker.is_some())
        .on_close(|s: &mut App| s.due_picker = None);
    if let Some(month) = s.due_picker {
        pop = pop.child(calendar(s, &task.id, due, month, k));
    }
    row().key("due-row").child(pop.build(k, layer))
}

fn calendar(s: &App, id: &str, due: Option<chrono::NaiveDate>, month: chrono::NaiveDate, k: &Kit) -> El {
    use chrono::Datelike;
    const CELL: f32 = 34.0;
    let pick = |label: &str, key: &str, day: chrono::NaiveDate| {
        let id = id.to_string();
        Button::new(label)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut App| s.set_due(id.clone(), Some(day)))
            .build(k)
            .key(key.to_string())
    };
    let quick = row()
        .key("due-quick")
        .gap(6.0)
        .child(pick("Today", "due-today", s.today))
        .child(pick("Tomorrow", "due-tomorrow", dates::add_days(s.today, 1)))
        .child(pick("Next week", "due-next-week", dates::next_week(s.today)));
    let nav = |glyph: &'static Icon, label: &str, key: &str, by: i32| {
        IconButton::new(glyph)
            .label(label)
            .variant(IconButtonVariant::Ghost)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut App| s.due_picker = s.due_picker.map(|m| dates::shift_month(m, by)))
            .build(k)
            .key(key.to_string())
    };
    let header = row()
        .key("due-month")
        .w_full()
        .center_y()
        .child(text(month.format("%B %Y").to_string()).font(k.ty.ui_medium).color(k.c.text_base).grow(1.0).pl(4.0))
        .child(nav(&PREV, "Previous month", "due-prev", -1))
        .child(nav(&NEXT, "Next month", "due-next", 1));
    let weekdays = row().key("due-weekdays").gap(2.0).children(dates::WEEKDAYS.iter().map(|w| {
        row()
            .key(format!("{w}"))
            .w(CELL)
            .center()
            .child(text(w.to_string()[..2].to_string()).font(k.ty.small).color(k.c.text_faint))
    }));
    let mut grid = col().key("due-grid").gap(2.0).child(weekdays);
    for (i, week) in dates::weeks(month).into_iter().enumerate() {
        let mut r = row().key(format!("week-{i}")).gap(2.0);
        for (j, day) in week.into_iter().enumerate() {
            r = r.child(match day {
                None => el().key(format!("blank-{j}")).w(CELL).h(28.0),
                Some(day) => {
                    let variant = if Some(day) == due {
                        ButtonVariant::Submit
                    } else if day == s.today {
                        ButtonVariant::Outline
                    } else {
                        ButtonVariant::Ghost
                    };
                    let id = id.to_string();
                    Button::new(day.day().to_string())
                        .variant(variant)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut App| s.set_due(id.clone(), Some(day)))
                        .build(k)
                        .key(format!("day-{}", day.day()))
                        .w(CELL)
                }
            });
        }
        grid = grid.child(r);
    }
    let mut c = col().key("due-calendar").gap(10.0).child(quick).child(header).child(grid);
    if due.is_some() {
        let id = id.to_string();
        c = c.child(
            Button::new("Remove due date")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut App| s.set_due(id.clone(), None))
                .build(k)
                .key("due-remove"),
        );
    }
    c
}

// The delete confirmation.

fn confirm_delete(s: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(id) = &s.confirm_delete else { return };
    let Some(list) = s.todos.lists().iter().find(|l| &l.id == id) else { return };
    let count = match s.todos.tasks_of(id) {
        Some(Load::Ready(t)) if !t.is_empty() => format!(" and its {} task{}", t.len(), if t.len() == 1 { "" } else { "s" }),
        _ => String::new(),
    };
    let id = id.clone();
    let buttons = row()
        .key("confirm-buttons")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(8.0)
        .child(el().grow(1.0))
        .child(Button::new("Cancel").on_press(|s: &mut App| s.confirm_delete = None).build(k).key("cancel"))
        .child(
            Button::new("Delete list")
                .variant(ButtonVariant::Danger)
                .on_press(move |s: &mut App| {
                    s.confirm_delete = None;
                    s.save_open_task();
                    s.details.set_open(false);
                    s.todo(Event::DeleteList(id.clone()));
                })
                .build(k)
                .key("confirm"),
        );
    Dialog::new("delete-list", "Delete this list?")
        .description(format!("\u{201c}{}\u{201d}{count} will be deleted. This can't be undone.", list.title))
        .open(true)
        .fit()
        .width(420.0)
        .child(buttons)
        .on_close(|s: &mut App| s.confirm_delete = None)
        .build(k, layer);
}
