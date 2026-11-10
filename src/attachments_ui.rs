//! The attachments section, the same under a task's details and a note: the files, uploads on
//! their way (with progress) or stopped (with why), and an Attach button. Files dropped on the
//! task's details or the note page attach too ([`drop_target`]).

use rune::{El, Icon, Tx, col, el, icon, row, text};
use rune_kit::glyphs::{CLOSE, FILE, FILE_IMAGE, FILE_TEXT};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, MenuItem, Progress, Spinner, context_menu,
};

use crate::api::{ApiError, Attachment};
use crate::attachments::{Owner, Upload, attachable, size_label};
use crate::todos::Load;
use crate::todos_ui::PLUS;
use crate::{App, MenuTarget};

pub fn section(s: &App, owner: &Owner, k: &Kit, layer: &mut Vec<El>) -> El {
    let can = attachable(owner);
    let files: &[Attachment] = match s.files.of(owner) {
        Some(Load::Ready(list)) => list,
        _ => &[],
    };
    let uploads: Vec<&Upload> = s.files.uploads_of(owner).collect();

    let mut head = row()
        .key("files-head")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(text("Attachments").font(k.ty.small_medium).color(k.c.text_muted));
    if !files.is_empty() {
        head = head.child(text(files.len().to_string()).key("files-count").font(k.ty.small).color(k.c.text_faint));
    }
    let attach = {
        let owner = owner.clone();
        Button::new("Attach")
            .icon(&PLUS)
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .disabled(!can)
            .help(if can { "Attach a file (or drop files here)" } else { "Saving… files can be attached in a moment" })
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.pick_attachment(owner.clone(), tx))
            .build(k)
            .key("files-attach")
    };
    head = head.child(el().grow(1.0)).child(attach);

    let mut list = col().key("files-list").w_full().gap(2.0);
    match s.files.of(owner) {
        Some(Load::Loading) => {
            list = list.child(row().key("files-loading").w_full().py(6.0).center().child(Spinner::new().build(k)));
        }
        Some(Load::Failed(e)) => {
            list = list.child(hint(failed_text(e), k).key("files-failed"));
        }
        _ => {}
    }
    for a in files {
        list = list.child(file_row(s, owner, a, k, layer));
    }
    for u in &uploads {
        list = list.child(upload_row(u, k));
    }
    if files.is_empty() && uploads.is_empty() && matches!(s.files.of(owner), Some(Load::Ready(_))) {
        list = list.child(hint("Drop files here, or use Attach.", k).key("files-empty"));
    }
    if let Some(n) = &s.files.notice {
        list = list.child(
            row()
                .key("files-notice")
                .w_full()
                .gap(8.0)
                .center_y()
                .child(text(n.clone()).font(k.ty.small).color(k.c.danger.fg).wrap().grow(1.0).shrink())
                .child(dismiss("Dismiss", |s: &mut App| s.files.notice = None, k).key("files-notice-close")),
        );
    }

    let mut section = col().key("files").w_full().gap(6.0).radius(k.radius.md).child(head).child(list);
    if s.drop_over && can {
        section = section.fill(k.c.text_accent.alpha(0.08)).border(1.0, k.c.text_accent.alpha(0.5));
    }
    section
}

/// Makes `pane` take dropped files for `owner`, and light up the section while they hover.
pub fn drop_target(pane: El, owner: &Owner) -> El {
    if !attachable(owner) {
        return pane;
    }
    let owner = owner.clone();
    pane.on_drop_files(move |s: &mut App, paths, _: &mut Tx| {
        s.drop_over = false;
        s.attach(owner.clone(), paths);
    })
    .on_drop_hover(|s: &mut App, over, _: &mut Tx| s.drop_over = over)
}

fn file_row(s: &App, owner: &Owner, a: &Attachment, k: &Kit, layer: &mut Vec<El>) -> El {
    let busy = s.files.busy.contains(&a.id);
    let lead = if busy { Spinner::new().build(k) } else { icon(file_icon(a), 16.0).color(k.c.icon_muted) };
    let remove = {
        let (owner, id) = (owner.clone(), a.id.clone());
        IconButton::new(&CLOSE)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("Remove {}", a.name))
            .disabled(busy)
            .on_press(move |s: &mut App| s.remove_attachment(&owner, &id))
            .build(k)
            .key("remove")
    };
    let wash = k.c.text_base.alpha(if k.dark { 0.06 } else { 0.05 });
    let r = row()
        .key(a.id.clone())
        .w_full()
        .px(6.0)
        .py(5.0)
        .gap(10.0)
        .center_y()
        .radius(k.radius.md)
        .hover(wash)
        .child(row().key("lead").w(16.0).center().child(lead))
        .child(
            col()
                .key("text")
                .grow(1.0)
                .shrink()
                .child(text(a.name.clone()).key("name").font(k.ty.ui).color(k.c.text_base).truncate_middle())
                .child(text(size_label(a.size)).key("size").font(k.ty.small).color(k.c.text_faint)),
        )
        .child(remove)
        .on_press({
            let (owner, id) = (owner.clone(), a.id.clone());
            move |s: &mut App| s.open_attachment(&owner, &id)
        })
        .on_secondary_press({
            let id = a.id.clone();
            move |s: &mut App, p, _: &mut Tx| s.menu = Some((MenuTarget::Attachment(id.clone()), p))
        });
    let point = match &s.menu {
        Some((MenuTarget::Attachment(m), p)) if *m == a.id => Some(*p),
        _ => None,
    };
    let (o1, o2, o3) = (owner.clone(), owner.clone(), owner.clone());
    let (i1, i2, i3) = (a.id.clone(), a.id.clone(), a.id.clone());
    context_menu(format!("file-menu-{}", a.id), r, point)
        .item(MenuItem::new("Open").on_select(move |s: &mut App| {
            s.menu = None;
            s.open_attachment(&o1, &i1);
        }))
        .item(MenuItem::new("Save as…").on_select_tx(move |s: &mut App, tx: &mut Tx| {
            s.menu = None;
            s.save_attachment(&o2, &i2, tx);
        }))
        .separator()
        .item(MenuItem::new("Remove").danger(true).on_select(move |s: &mut App| {
            s.menu = None;
            s.remove_attachment(&o3, &i3);
        }))
        .on_close(|s: &mut App| s.menu = None)
        .build(k, layer)
        .key(a.id.clone())
}

fn upload_row(u: &Upload, k: &Kit) -> El {
    let key = u.key;
    let detail = match &u.failed {
        Some(why) => text(why.clone()).key("why").font(k.ty.small).color(k.c.danger.fg).wrap(),
        None => Progress::new(Some(u.progress())).build(k).key("progress").mt(4.0),
    };
    let mut r = row()
        .key(format!("upload-{key}"))
        .w_full()
        .px(6.0)
        .py(5.0)
        .gap(10.0)
        .center_y()
        .child(row().key("lead").w(16.0).center().child(icon(&FILE, 16.0).color(k.c.icon_muted)))
        .child(
            col()
                .key("text")
                .grow(1.0)
                .shrink()
                .child(text(u.name.clone()).key("name").font(k.ty.ui).color(k.c.text_muted).truncate_middle())
                .child(detail),
        );
    if u.failed.is_some() {
        r = r.child(dismiss("Dismiss", move |s: &mut App| s.files.dismiss_upload(key), k).key("dismiss"));
    }
    r
}

fn dismiss(label: &str, f: impl Fn(&mut App) + 'static, k: &Kit) -> El {
    IconButton::new(&CLOSE)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(label)
        .on_press(f)
        .build(k)
}

fn hint(t: impl Into<String>, k: &Kit) -> El {
    text(t.into()).font(k.ty.small).color(k.c.text_faint).wrap()
}

fn failed_text(e: &ApiError) -> String {
    match e {
        ApiError::AttachmentsOff => e.message().to_string(),
        e => format!("Couldn't load the attachments. {}", e.message()),
    }
}

fn file_icon(a: &Attachment) -> &'static Icon {
    let mime = a.mime.as_str();
    if mime.starts_with("image/") {
        &FILE_IMAGE
    } else if mime.starts_with("text/") || mime == "application/pdf" || mime.contains("document") {
        &FILE_TEXT
    } else {
        &FILE
    }
}
