//! The sign-in screen, built from rune-kit components on the OpenCode theme.

use std::sync::OnceLock;

use rune::{Color, Cx, El, Image, ImageCache, TextStyle, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Spinner, TextInput};

use crate::App;
use crate::auth::{Auth, Event, FormError, Mode};
use crate::session::Session;

const CARD_WIDTH: f32 = 380.0;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let content = match &s.auth {
        Auth::Checking { .. } => checking(k),
        Auth::SignedIn { session } => return crate::todos_ui::view(s, session, k, cx),
        Auth::SignedOut { error } => form(s, error.as_ref(), false, k),
        Auth::Submitting { .. } => form(s, None, true, k),
    };
    // Reduce Motion freezes the background on one frame, and `poll` stops asking for frames.
    s.still.set(cx.system_reduced_motion());
    let t = if s.still.get() { 0.0 } else { s.t };
    let size = cx.layout_size("screen").unwrap_or([960.0, 680.0]);
    let screen = col()
        .key("screen")
        .w_full()
        .h_full()
        .center()
        .fill(k.c.bg_base)
        .child(crate::backdrop::backdrop(t, size, k))
        .child(content);
    rune_kit::root(screen, [])
}

fn card(k: &Kit) -> El {
    // Frosted glass over the moving background.
    col()
        .key("card")
        .w(CARD_WIDTH)
        .padding(28.0)
        .gap(20.0)
        .radius(k.radius.xl)
        .backdrop_blur(24.0)
        .fill(k.c.surface_raised.alpha(if k.dark { 0.72 } else { 0.78 }))
        .border(1.0, k.c.border_base)
        .shadow(12.0, 32.0, Color::BLACK.alpha(if k.dark { 0.35 } else { 0.08 }))
}

/// The account avatar's gradient.
pub const LOGO_FROM: Color = Color { r: 0.39, g: 0.40, b: 0.95, a: 1.0 }; // indigo
pub const LOGO_TO: Color = Color { r: 0.66, g: 0.33, b: 0.97, a: 1.0 }; // violet

/// The app icon: the cairn on its dark tile (`assets/icon`, built by `scripts/make_icons.py`).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon/icon-256.png");
/// The icon's tile colour, shown while the image decodes.
const TILE: Color = Color { r: 0.227, g: 0.227, b: 0.227, a: 1.0 };

/// The decoded icon, and the cache that keeps it (loaded once, in `setup`).
static LOGO: OnceLock<(ImageCache, Image)> = OnceLock::new();

/// Starts decoding the logo; `wait` blocks until it's ready (headless snapshots).
pub fn load_logo(waker: rune::Waker, wait: bool) {
    let logo = LOGO.get_or_init(|| {
        let cache = ImageCache::new(waker);
        let image = cache.load(ICON_PNG.to_vec());
        (cache, image)
    });
    if wait {
        let _ = logo.1.wait();
    }
}

pub fn logo(size: f32) -> El {
    // The tile colour shows until the image decodes; its pixels then paint over it by
    // themselves (a pending image draws nothing and wakes the window when ready).
    let tile = el().key("logo").size(size, size).radius(size * 0.225).fill(TILE);
    match LOGO.get() {
        Some((_, image)) => tile.pixels(image.pixels()),
        None => tile,
    }
}

fn brand(k: &Kit) -> El {
    row()
        .key("brand")
        .gap(10.0)
        .center_y()
        .child(logo(32.0))
        .child(text("Todo & Notes").font(k.ty.ui_medium).color(k.c.text_base))
}

fn heading(title: &str, subtitle: &str, k: &Kit) -> El {
    col()
        .gap(6.0)
        .child(brand(k).mb(10.0))
        .child(text(title.to_string()).font(TextStyle::new(22.0, 600.0)).color(k.c.text_strong))
        .child(text(subtitle.to_string()).font(k.ty.ui).color(k.c.text_muted))
}

fn field(label: &str, input: El, k: &Kit) -> El {
    col().gap(6.0).child(text(label.to_string()).font(k.ty.ui_medium).color(k.c.text_base)).child(input)
}

fn form(s: &App, error: Option<&FormError>, busy: bool, k: &Kit) -> El {
    let sign_up = s.mode == Mode::SignUp;
    let (title, subtitle) = if sign_up {
        ("Create your account", "Keep your todos and notes in one place.")
    } else {
        ("Welcome back", "Sign in to your todos and notes.")
    };

    let email = TextInput::new(&s.email)
        .label("Email")
        .placeholder("you@example.com")
        .width(None)
        .large()
        .invalid(error.is_some_and(FormError::on_email))
        .disabled(busy)
        .on_edit(|s: &mut App| &mut s.email)
        .on_change(|s: &mut App, _: &mut Tx| s.dispatch(Event::Edited))
        .on_submit(|_: &mut App, tx: &mut Tx| tx.focus("password", false))
        .build(k)
        .key("email");

    let reveal = Button::new(if s.reveal { "Hide" } else { "Show" })
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .disabled(busy)
        .on_press(|s: &mut App| s.reveal = !s.reveal)
        .build(k)
        .key("reveal");

    let password = TextInput::new(&s.password)
        .secure()
        .revealed(s.reveal)
        .label("Password")
        .placeholder(if sign_up { "At least 8 characters" } else { "Your password" })
        .width(None)
        .large()
        .trailing(reveal)
        .invalid(error.is_some_and(FormError::on_password))
        .disabled(busy)
        .on_edit(|s: &mut App| &mut s.password)
        .on_change(|s: &mut App, _: &mut Tx| s.dispatch(Event::Edited))
        .on_submit(|s: &mut App, _: &mut Tx| s.submit())
        .build(k)
        .key("password");

    let label = match (sign_up, busy) {
        (false, false) => "Sign in",
        (false, true) => "Signing in…",
        (true, false) => "Create account",
        (true, true) => "Creating account…",
    };
    let submit = Button::new(label)
        .variant(ButtonVariant::Submit)
        .control_size(ControlSize::Large)
        .rolling("submit-label")
        .disabled(busy)
        .on_press(|s: &mut App| s.submit())
        .build(k)
        .key("submit")
        .w_full();

    let (prompt, action) =
        if sign_up { ("Already have an account?", "Sign in") } else { ("New here?", "Create an account") };
    let switch = row()
        .key("switch-row")
        .gap(2.0)
        .center()
        .child(text(prompt).font(k.ty.ui).color(k.c.text_muted))
        .child(
            Button::new(action)
                .variant(ButtonVariant::Ghost)
                .disabled(busy)
                .on_press(|s: &mut App| s.switch_mode())
                .build(k)
                .key("switch-mode"),
        );

    let mut fields = col().gap(14.0).child(field("Email", email, k)).child(field("Password", password, k));
    if let Some(e) = error {
        fields = fields.child(text(e.message()).key("error").font(k.ty.small).color(k.c.danger.fg));
    }

    card(k).child(heading(title, subtitle, k)).child(fields).child(submit).child(switch)
}

fn checking(k: &Kit) -> El {
    col()
        .key("checking")
        .gap(16.0)
        .items_center()
        .child(logo(56.0))
        .child(Spinner::new().build(k))
        .child(text("Signing you in…").font(k.ty.ui).color(k.c.text_muted))
}

/// Signs a scene in with sample notes, the first open.
fn sample_notes(s: &mut App, preview: bool) {
    use crate::api::Note;
    sample_todos(s, 0);
    s.section = crate::Section::Notes;
    let note = |id: &str, title: &str, body: &str, pinned: bool, day: &str| Note {
        id: id.into(),
        title: title.into(),
        body: body.into(),
        pinned,
        created_at: format!("2026-11-{day}T09:00:00Z"),
        updated_at: format!("2026-11-{day}T09:00:00Z"),
    };
    let notes = vec![
        note(
            "n1",
            "Trip packing list",
            "## Clothes\n- [x] Rain jacket\n- [ ] Hiking boots\n- [ ] Two warm layers\n\n## Gear\n- [ ] Headlamp and spare batteries\n- [x] First-aid kit\n\n> Check the cabin has bedding before packing sleeping bags.",
            false,
            "08",
        ),
        note("n2", "Book ideas", "A story told entirely through **grocery lists** left on a fridge.", false, "06"),
        note("n3", "Meeting notes: Q4 planning", "Agreed to ship notes before the holidays.\n\n1. Notes backend\n2. Notes UI\n3. Sync", false, "03"),
        note("n4", "Wi-Fi and door codes", "Studio: `studio-5g`\nGate: ask Sam", true, "01"),
    ];
    s.note(crate::notes::Event::Reply(crate::todos::Reply::Notes(Ok(notes))));
    s.note(crate::notes::Event::Select("n1".into()));
    s.note_preview = preview;
}

/// Signs a scene in with sample lists, without a server.
fn sample_todos(s: &mut App, list: usize) {
    use crate::api::{Task, TaskList};
    use crate::todos::{Event as T, Reply, step};
    s.auth = Auth::SignedIn {
        session: Session { email: "taha@example.com".into() },
    };
    let lists = [("l1", "Groceries"), ("l2", "Work"), ("l3", "Weekend trip"), ("l4", "Empty list")];
    let tasks: &[(&str, &str, &str, bool, &str)] = &[
        ("t1", "l1", "Oat milk", false, ""),
        ("t2", "l1", "Sourdough bread", false, "From the bakery on 5th, not the supermarket."),
        ("t3", "l1", "Eggs (a dozen)", false, ""),
        ("t4", "l1", "Coffee beans", true, ""),
        ("t5", "l1", "Spinach", true, ""),
        ("t6", "l2", "Review the auth PR", false, ""),
        ("t7", "l2", "Write the release notes", false, ""),
        ("t8", "l3", "Book the cabin", true, ""),
    ];
    // Due days around today, so the labels read the same whenever the scene is taken.
    let today = s.today;
    let due = move |id: &str| {
        let day = match id {
            "t1" => today.pred_opt()?,
            "t3" => today,
            "t6" => crate::dates::add_days(today, 1),
            "t7" => crate::dates::add_days(today, 4),
            "t2" => crate::dates::add_days(today, 12),
            _ => return None,
        };
        Some(crate::dates::stamp(day))
    };
    step(&mut s.todos, T::Open);
    let to_list = |(id, title): &(&str, &str)| TaskList {
        id: id.to_string(),
        title: title.to_string(),
        created_at: "2026-11-01T09:00:00Z".into(),
    };
    step(&mut s.todos, T::Reply(Reply::Lists(Ok(lists.iter().map(to_list).collect()))));
    for (lid, _) in &lists {
        let mine = tasks
            .iter()
            .filter(|t| t.1 == *lid)
            .map(|&(id, l, title, done, notes)| Task {
                id: id.into(),
                list_id: l.into(),
                title: title.into(),
                description: notes.into(),
                due_at: due(id),
                done,
                created_at: "2026-11-02T10:00:00Z".into(),
            })
            .collect();
        step(&mut s.todos, T::Select(lid.to_string()));
        step(&mut s.todos, T::Reply(Reply::Tasks { list: lid.to_string(), result: Ok(mine) }));
    }
    step(&mut s.todos, T::Select(lists[list].0.to_string()));
}

/// Fixed states for headless screenshots: `--scene <name>`.
pub const SCENES: &[(&str, fn(&mut App))] = &[
    ("sign-in", |_| {}),
    ("sign-up", |s| s.mode = Mode::SignUp),
    ("error", |s| {
        s.email.replace_text("taha@example.com");
        s.password.replace_text("wrong-password");
        s.auth = Auth::SignedOut {
            error: Some(FormError::Api(crate::api::ApiError::InvalidCredentials)),
        };
    }),
    ("offline", |s| {
        s.auth = Auth::SignedOut { error: Some(FormError::Api(crate::api::ApiError::Unreachable)) };
    }),
    ("submitting", |s| {
        s.email.replace_text("taha@example.com");
        s.password.replace_text("password");
        s.auth = Auth::Submitting { attempt: Default::default(), mode: Mode::SignIn };
    }),
    ("checking", |s| {
        s.auth = Auth::Checking {
            attempt: Default::default(),
            session: Session { email: "taha@example.com".into() },
        };
    }),
    ("todos", |s| sample_todos(s, 0)),
    ("todos-details", |s| {
        sample_todos(s, 0);
        s.open_task("t2".into());
    }),
    ("todos-due-picker", |s| {
        sample_todos(s, 0);
        s.open_task("t2".into());
        s.toggle_due_picker();
    }),
    ("update-ready", |s| {
        sample_todos(s, 0);
        s.updates = crate::updates::Updates::showing(crate::updates::Status::Ready("0.2.0".into()));
    }),
    ("todos-due-empty", |s| {
        sample_todos(s, 0);
        s.open_task("t4".into());
        s.toggle_due_picker();
    }),
    ("todos-empty-list", |s| sample_todos(s, 3)),
    ("todos-delete", |s| {
        sample_todos(s, 0);
        s.confirm_delete = Some("l1".into());
    }),
    ("todos-notice", |s| {
        sample_todos(s, 1);
        s.todos.notice = Some("Couldn't save your change. Can't reach the server.".into());
    }),
    ("notes", |s| sample_notes(s, false)),
    ("notes-preview", |s| sample_notes(s, true)),
    ("notes-none", |s| {
        sample_todos(s, 0);
        s.section = crate::Section::Notes;
        s.note(crate::notes::Event::Reply(crate::todos::Reply::Notes(Ok(vec![]))));
    }),
    ("todos-none", |s| {
        sample_todos(s, 0);
        let lists = s.todos.lists().iter().map(|l| l.id.clone()).collect::<Vec<_>>();
        for id in lists {
            s.todo(crate::todos::Event::DeleteList(id));
        }
    }),
];
