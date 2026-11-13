//! The todo screen: lists in the sidebar, the selected list's tasks in the middle, and the open
//! task's details on the right. rune-kit's shell components on the OpenCode theme.

use rune::{Cx, El, Icon, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::shell::{self, SidebarToggle};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant, Kit, MenuItem, Placement, Popover,
    SegmentedControl, Select, Sidebar, SidebarRow, SidebarSection, Spinner, SplitView, TextArea, TextInput, Toast,
    ToastRegion, Toolbar, context_menu,
};

use crate::api::{Task, TaskList};
use crate::dates;
use crate::updates::Status;
use crate::session::Session;
use crate::todos::{Event, Load, Place};
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

static UP: Icon = Icon {
    name: "todo-up",
    view: 16.0,
    body: r#"<path d="M4 10L8 6L12 10" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static DOWN: Icon = Icon {
    name: "todo-down",
    view: 16.0,
    body: r#"<path d="M4 6L8 10L12 6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static TRASH: Icon = Icon {
    name: "todo-trash",
    view: 16.0,
    body: r#"<path d="M3.5 4.75H12.5M6.5 4.75V3.25H9.5V4.75M4.75 4.75L5.25 13H10.75L11.25 4.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
pub(crate) static WORKFLOW: Icon = Icon {
    name: "todo-workflow",
    view: 16.0,
    body: r#"<circle cx="3.5" cy="8" r="1.75" stroke="currentColor" stroke-width="1.2" fill="none"/><circle cx="8" cy="8" r="1.75" fill="currentColor"/><circle cx="12.5" cy="8" r="1.75" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M5.25 8H6.25M9.75 8H10.75" stroke="currentColor" stroke-width="1.2"/>"#,
};
static RING: Icon = Icon {
    name: "todo-status-ring",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.25" stroke="currentColor" stroke-width="1.5" fill="none"/>"#,
};
static DOT: Icon = Icon {
    name: "todo-status-dot",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="7" fill="currentColor"/>"#,
};
static TICK: Icon = Icon {
    name: "todo-status-tick",
    view: 16.0,
    body: r#"<path d="M5 8.25L7.1 10.25L11 6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The colours a step can have, in the order a new step takes them. No green: that's done.
pub(crate) const STAGE_COLORS: [&str; 8] =
    ["#f59e0b", "#3b82f6", "#8b5cf6", "#ec4899", "#14b8a6", "#ef4444", "#64748b", "#a16207"];

/// A pie slice filling `frac` of the status ring, clockwise from the top (quantised to 1/24;
/// icons are static, so each size is made once).
fn wedge(frac: f32) -> &'static Icon {
    use std::sync::Mutex;
    static CACHE: Mutex<Vec<Option<&'static Icon>>> = Mutex::new(Vec::new());
    let step = (frac.clamp(0.0, 1.0) * 24.0).round() as usize;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.resize(25, None);
    if let Some(i) = cache[step] {
        return i;
    }
    let a = step as f32 / 24.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    let r = 4.0;
    let (x, y) = (8.0 + r * a.cos(), 8.0 + r * a.sin());
    let large = u8::from(step > 12);
    let body: &'static str = match step {
        0 => "",
        24 => r#"<circle cx="8" cy="8" r="4" fill="currentColor"/>"#,
        _ => Box::leak(format!(r#"<path d="M8 8V4A4 4 0 {large} 1 {x:.3} {y:.3}Z" fill="currentColor"/>"#).into_boxed_str()),
    };
    let i: &'static Icon = Box::leak(Box::new(Icon { name: "todo-status-wedge", view: 16.0, body }));
    cache[step] = Some(i);
    i
}

/// A step's colour (the accent when it doesn't parse).
pub(crate) fn stage_color(hex: &str, k: &Kit) -> rune::Color {
    rune::color::hex(hex).unwrap_or(k.c.text_accent)
}

/// The colour a place shows in: grey for to do, the step's colour, green for done.
fn place_color(s: &App, p: &Place, k: &Kit) -> rune::Color {
    match p {
        Place::Todo => k.c.icon_muted,
        Place::Stage(id) => s.todos.stage(id).map_or(k.c.icon_muted, |st| stage_color(&st.color, k)),
        Place::Done => k.c.success.fg,
    }
}

/// The circle that shows where a task is: empty for to do, filling up through the steps in the
/// step's colour, and a green tick for done.
pub(crate) fn status_icon(s: &App, p: &Place, size: f32, k: &Kit) -> El {
    let color = place_color(s, p, k);
    let base = el().size(size, size).a11y_hidden();
    match p {
        Place::Done => base
            .child(icon(&DOT, size).absolute(0.0, 0.0).color(color))
            .child(icon(&TICK, size).absolute(0.0, 0.0).color(rune::Color::WHITE)),
        Place::Todo => base.child(icon(&RING, size).absolute(0.0, 0.0).color(color)),
        Place::Stage(id) => {
            let n = s.todos.stages.len() as f32;
            let at = s.todos.stages.iter().position(|st| &st.id == id).unwrap_or(0) as f32 + 1.0;
            base.child(icon(&RING, size).absolute(0.0, 0.0).color(color))
                .child(icon(wedge(at / (n + 1.0)), size).absolute(0.0, 0.0).color(color))
        }
    }
}

/// A task's status circle as a button: a click moves it on a place.
fn status_button(s: &App, t: &Task, k: &Kit) -> El {
    let here = s.todos.place(t);
    let next = s.todos.next_place(&here);
    let (here_name, next_name) = (s.todos.place_name(&here), s.todos.place_name(&next));
    let id = t.id.clone();
    row()
        .key("status")
        .size(24.0, 24.0)
        .mr(8.0)
        .center()
        .radius(12.0)
        .role("button")
        .label(format!("{here_name}. Move to {next_name}"))
        .help(format!("{here_name} \u{2014} click to move to {next_name}"))
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .hover(k.c.hover)
        .on_press(move |s: &mut App| s.todo(Event::Advance(id.clone())))
        .child(status_icon(s, &here, 18.0, k))
}

/// The signed-in screen and its overlays (menus, the delete confirmation).
pub fn view(s: &App, session: &Session, k: &Kit, cx: &mut Cx<App>) -> El {
    let mut layer = Vec::new();
    let split = match s.section {
        Section::Tasks => tasks_split(s, session, k, cx, &mut layer),
        Section::Notes => crate::notes_ui::split(s, session, k, cx, &mut layer),
    };
    confirm_delete(s, k, &mut layer);
    workflow_dialog(s, k, &mut layer);
    if s.section == Section::Tasks {
        if let Some(t) = undo_toast(s, k) {
            layer.push(ToastRegion::new().toast(t).build(k));
        }
    }
    rune_kit::root(split, layer)
}

/// "Moved to To show", with Undo, after a task moves.
fn undo_toast(s: &App, k: &Kit) -> Option<El> {
    let m = s.todos.moved.as_ref()?;
    let task = s.todos.task(&m.task)?;
    Some(
        Toast::new("undo-move", format!("Moved to {}", s.todos.place_name(&m.to)))
            .description(task.title.clone())
            .lead(status_icon(s, &m.to, 16.0, k))
            .action(k, "Undo", true, |s: &mut App, _: &mut Tx| s.todo(Event::Undo))
            .on_close(|s: &mut App| s.todo(Event::DismissUndo))
            .build(k),
    )
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
    let sb = Sidebar::new()
        .header_padding(4.0, 8.0)
        .header(brand(k))
        .header(row().key("section-row").w_full().px(4.0).pb(8.0).child(pick));
    let sb = match update_ready(s, k) {
        Some(banner) => sb.footer(banner),
        None => sb,
    };
    sb.footer(account(session, k))
}

/// "Version x is ready", with a button to restart into it, once an update has downloaded.
fn update_ready(s: &App, k: &Kit) -> Option<El> {
    let Status::Ready(version) = s.updates.status() else { return None };
    Some(
        row()
            .key("update")
            .w_full()
            .px(8.0)
            .py(6.0)
            .gap(8.0)
            .center_y()
            .child(
                col()
                    .grow(1.0)
                    .shrink()
                    .child(text(format!("Version {version} is ready")).font(k.ty.small_medium).color(k.c.text_base))
                    .child(text("Restart to update").font(k.ty.small).color(k.c.text_muted)),
            )
            .child(
                Button::new("Restart")
                    .variant(ButtonVariant::Submit)
                    .control_size(ControlSize::Small)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.restart_to_update(tx))
                    .build(k)
                    .key("update-restart"),
            ),
    )
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
    let workflow = SidebarRow::new("Workflow")
        .icon(&WORKFLOW)
        .detail(if s.todos.stages.is_empty() { String::new() } else { format!("{} steps", s.todos.stages.len()) })
        .on_select(|s: &mut App, _: &mut Tx| s.open_workflow())
        .build(k)
        .key("workflow");
    let sb = sidebar_frame(s, session, k).label("Lists").header(new_list).header(workflow);
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
    // The tasks in each place, in the workflow's order: to do, the steps, done.
    let groups: Vec<(Place, Vec<&Task>)> = match tasks {
        Some(Load::Ready(ts)) => s
            .todos
            .places()
            .into_iter()
            .map(|p| {
                let here = ts.iter().filter(|t| s.todos.place(t) == p).collect();
                (p, here)
            })
            .collect(),
        _ => Vec::new(),
    };
    let count = |p: &Place| groups.iter().find(|g| &g.0 == p).map_or(0, |g| g.1.len());
    let (open, done) = (count(&Place::Todo), count(&Place::Done));
    let summary = match tasks {
        Some(Load::Ready(ts)) if ts.is_empty() => "No tasks yet".to_string(),
        Some(Load::Ready(ts)) if done == ts.len() => format!("All {done} done"),
        Some(Load::Ready(_)) => {
            let mut parts = Vec::new();
            for (p, here) in &groups {
                match p {
                    Place::Todo if open > 0 || done == 0 => parts.push(format!("{open} to do")),
                    Place::Stage(_) if !here.is_empty() => {
                        parts.push(format!("{} {}", here.len(), s.todos.place_name(p).to_lowercase()))
                    }
                    Place::Done if done > 0 => parts.push(format!("{done} done")),
                    _ => {}
                }
            }
            parts.join(" · ")
        }
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
        Some(Load::Ready(ts)) => {
            let mut c = content;
            if ts.is_empty() {
                c = c.child(
                    text("Nothing here yet. Add your first task above.")
                        .key("no-tasks")
                        .font(k.ty.ui)
                        .color(k.c.text_faint)
                        .mt(8.0),
                );
            }
            for (p, here) in &groups {
                if here.is_empty() {
                    continue;
                }
                c = c.child(match p {
                    // To do sits right under the add field, with no header.
                    Place::Todo => col().key("open").w_full().gap(2.0).children(here.iter().map(|t| task_row(s, t, k, layer))),
                    _ => place_section(s, p, here, k, layer),
                });
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

/// A step's or done's tasks under a header that folds them away: "▾ ◔ To show · 3".
fn place_section(s: &App, p: &Place, tasks: &[&Task], k: &Kit, layer: &mut Vec<El>) -> El {
    let key = match p {
        Place::Stage(id) => format!("stage-{id}"),
        _ => "done".to_string(),
    };
    let open = !s.collapsed.contains(p);
    let name = if *p == Place::Done { "Completed".to_string() } else { s.todos.place_name(p) };
    let toggle = p.clone();
    let header = row()
        .key(format!("{key}-header"))
        .role("button")
        .label(format!("{name}, {} tasks", tasks.len()))
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .px(8.0)
        .py(4.0)
        .mb(4.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.md)
        .hover(k.c.hover)
        .on_press(move |s: &mut App| {
            if !s.collapsed.remove(&toggle) {
                s.collapsed.insert(toggle.clone());
            }
        })
        .child(text(if open { "▾" } else { "▸" }).font(k.ty.small).color(k.c.text_faint))
        .child(status_icon(s, p, 14.0, k))
        .child(text(name).font(k.ty.small_medium).color(k.c.text_muted))
        .child(text(tasks.len().to_string()).font(k.ty.small).color(k.c.text_faint));
    let mut section = col().key(key).w_full().gap(2.0).child(row().key("header-row").w_full().child(header));
    if open {
        section = section.children(tasks.iter().map(|t| task_row(s, t, k, layer)));
    }
    section
}

fn task_row(s: &App, t: &Task, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = t.id.clone();
    let done = t.done;
    let here = s.todos.place(t);
    let check = status_button(s, t, k);
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
        .py(7.0)
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
    let mut menu = context_menu(format!("task-menu-{id}"), r, point)
        .item(MenuItem::new("Open details").on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.open_task(id.clone());
            }
        }))
        .separator();
    // Every other place, in order: jump straight to done, or back a step.
    for p in s.todos.places().into_iter().filter(|p| *p != here) {
        let label = match &p {
            Place::Done => "Mark as done".to_string(),
            Place::Todo if done => "Mark as not done".to_string(),
            _ => format!("Move to {}", s.todos.place_name(&p)),
        };
        let id = id.clone();
        menu = menu.item(MenuItem::new(label).on_select(move |s: &mut App| {
            s.menu = None;
            s.todo(Event::Move { id: id.clone(), to: p.clone() });
        }));
    }
    let menu = menu
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
    let owner = (crate::api::Parent::Task, task.id.clone());
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
    // Where the task is, and a picker for any other place.
    let places = s.todos.places();
    let names: Vec<String> = places.iter().map(|p| s.todos.place_name(p)).collect();
    let here = s.todos.place(task);
    let status = Select::new("task-stage", &names)
        .label("Status")
        .width(220.0)
        .current(Some(&s.todos.place_name(&here)))
        .open(s.stage_picker)
        .on_toggle(|s: &mut App| s.stage_picker = !s.stage_picker)
        .on_close(|s: &mut App| s.stage_picker = false)
        .on_choose({
            let id = id.clone();
            move |s: &mut App, name: &str, _: &mut Tx| {
                s.stage_picker = false;
                let to = s.todos.places().into_iter().find(|p| s.todos.place_name(p) == name);
                if let Some(to) = to {
                    s.todo(Event::Move { id: id.clone(), to });
                }
            }
        })
        .build(k, layer);
    let status = row().key("task-status").gap(10.0).center_y().child(status_icon(s, &here, 18.0, k)).child(status);
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
        .child(col().gap(6.0).child(label("Status")).child(status))
        .child(col().gap(6.0).child(label("Due")).child(due_picker(s, task, k, layer)))
        .child(col().gap(6.0).child(label("Notes")).child(notes))
        .child(crate::attachments_ui::section(s, &owner, k, layer))
        .child(actions);
    if let Some(day) = task.created_at.get(..10) {
        pane = pane.child(text(format!("Created {day}")).font(k.ty.small).color(k.c.text_faint));
    }
    crate::attachments_ui::drop_target(pane, &owner)
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

// The workflow editor.

/// The steps between to do and done: name, colour and order, and adding or removing them.
fn workflow_dialog(s: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(drafts) = &s.workflow else { return };
    let fixed = |key: &str, p: Place, name: &str, note: &str| {
        row()
            .key(key.to_string())
            .w_full()
            .px(4.0)
            .py(6.0)
            .gap(10.0)
            .center_y()
            .child(status_icon(s, &p, 18.0, k))
            .child(text(name.to_string()).font(k.ty.ui_medium).color(k.c.text_muted).grow(1.0))
            .child(text(note.to_string()).font(k.ty.small).color(k.c.text_faint))
    };
    let last = drafts.len().saturating_sub(1);
    let mut rows = col().key("workflow-steps").w_full().gap(4.0).child(fixed("fixed-todo", Place::Todo, "To do", "Always first"));
    for (i, d) in drafts.iter().enumerate() {
        let id = d.id.clone();
        let color = stage_color(&d.color, k);
        let swatch = row()
            .key("swatch")
            .size(28.0, 28.0)
            .center()
            .radius(6.0)
            .role("button")
            .label("Change colour")
            .help("Change colour")
            .focusable(true)
            .focus_ring(2.0, 0.0, k.c.border_focus)
            .hover(k.c.hover)
            .on_press({
                let id = id.clone();
                move |s: &mut App| s.cycle_draft_color(&id)
            })
            .child(el().size(16.0, 16.0).radius(8.0).fill(color));
        let name = TextInput::new(&d.name)
            .placeholder("Step name, like To show")
            .width(None)
            .on_edit({
                let id = id.clone();
                move |s: &mut App| {
                    let at = s.workflow.as_ref().and_then(|w| w.iter().position(|d| d.id == id));
                    match (at, s.workflow.as_mut()) {
                        (Some(i), Some(w)) => &mut w[i].name,
                        _ => &mut s.rename,
                    }
                }
            })
            .build(k)
            .key(format!("stage-name-{id}"));
        let button = |glyph: &'static Icon, label: &str, key: &str, enabled: bool, f: Box<dyn Fn(&mut App)>| {
            IconButton::new(glyph)
                .label(label)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .disabled(!enabled)
                .on_press(move |s: &mut App| f(s))
                .build(k)
                .key(key.to_string())
        };
        let (up, down, del) = (id.clone(), id.clone(), id.clone());
        rows = rows.child(
            row()
                .key(id.clone())
                .w_full()
                .gap(6.0)
                .center_y()
                .child(swatch)
                .child(el().key("name").grow(1.0).shrink().child(name))
                .child(button(&UP, "Move up", "up", i > 0, Box::new(move |s| s.move_draft(&up, -1))))
                .child(button(&DOWN, "Move down", "down", i < last, Box::new(move |s| s.move_draft(&down, 1))))
                .child(button(&TRASH, "Remove step", "remove", true, Box::new(move |s| s.remove_draft(&del)))),
        );
    }
    rows = rows.child(
        row().key("add-row").w_full().py(4.0).child(
            Button::new("Add a step")
                .variant(ButtonVariant::GhostMuted)
                .icon(&PLUS)
                .disabled(drafts.len() >= 12)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.add_draft(tx))
                .build(k)
                .key("add-step"),
        ),
    );
    rows = rows.child(fixed("fixed-done", Place::Done, "Done", "Always last"));
    let buttons = row()
        .key("workflow-buttons")
        .w_full()
        .pt(8.0)
        .gap(8.0)
        .child(el().grow(1.0))
        .child(Button::new("Cancel").on_press(|s: &mut App| s.workflow = None).build(k).key("cancel"))
        .child(
            Button::new("Save")
                .variant(ButtonVariant::Submit)
                .on_press(|s: &mut App| s.save_workflow())
                .build(k)
                .key("save"),
        );
    Dialog::new("workflow", "Workflow")
        .description("The steps a task goes through between To do and Done.")
        .open(true)
        .fit()
        .width(480.0)
        .child(
            col()
                .key("workflow-body")
                .w_full()
                .px(16.0)
                .pb(16.0)
                .gap(8.0)
                .child(rows)
                .child(
                    text("Click a task's circle to move it to the next step. Removing a step moves its tasks back one step.")
                        .key("workflow-hint")
                        .font(k.ty.small)
                        .color(k.c.text_faint)
                        .wrap()
                        .w_full(),
                )
                .child(buttons),
        )
        .on_close(|s: &mut App| s.workflow = None)
        .build(k, layer);
}
