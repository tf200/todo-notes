//! The notes screen: notes in the sidebar (pinned first), the open note's editor in the main
//! pane, with a Markdown preview whose task boxes tick in the note.

use rune::{Cx, El, ScrollSpec, TextStyle, Tx, col, el, row, text};
use rune_kit::shell::SidebarToggle;
use rune_kit::{
    Button, ButtonVariant, ControlSize, Kit, Markdown, MenuItem, SegmentedControl, SidebarRow, SidebarSection, Spinner,
    SplitView, TextArea, TextInput, Toolbar, context_menu,
};

use crate::api::Note;
use crate::notes::{Event, preview};
use crate::session::Session;
use crate::todos::Load;
use crate::todos_ui::{PLUS, centered, empty_state, sidebar_frame};
use crate::{App, MenuTarget};

static NOTE_ICON: &rune::Icon = &crate::todos_ui::NOTE;

pub fn split(s: &App, session: &Session, k: &Kit, cx: &mut Cx<App>, layer: &mut Vec<El>) -> El {
    let open = s.notes.open_note();
    let title = open.map_or("Notes", |_| if s.note_title.text.trim().is_empty() { "Untitled" } else { s.note_title.text.as_str() });
    let mut toolbar = Toolbar::new().title(title);
    if let Some(note) = open {
        toolbar = toolbar.trailing(toolbar_actions(s, note, k));
    }
    let main = main_pane(s, k, layer);
    SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar(s, session, k, layer))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(10.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().in_header())
        .reduced_motion(cx.is_reduced_motion())
        .build(k)
}

fn toolbar_actions(s: &App, note: &Note, k: &Kit) -> El {
    let id = note.id.clone();
    let pinned = note.pinned;
    row()
        .key("note-actions")
        .gap(8.0)
        .center_y()
        .child(
            Button::new(if pinned { "Unpin" } else { "Pin" })
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut App| s.note(Event::SetPinned { id: id.clone(), pinned: !pinned }))
                .build(k)
                .key("note-pin"),
        )
        .child(
            SegmentedControl::new(&["Write", "Preview"], Some(usize::from(s.note_preview)))
                .label("Editor mode")
                .fit()
                .on_pick(|s: &mut App, i, _: &mut Tx| {
                    s.save_open_note();
                    s.note_preview = i == 1;
                })
                .build(k)
                .key("note-mode"),
        )
}

// The sidebar.

fn sidebar(s: &App, session: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let new_note = SidebarRow::new("New note")
        .icon(&PLUS)
        .detail("Ctrl+N")
        .on_select(|s: &mut App, tx: &mut Tx| s.new_note(tx))
        .build(k)
        .key("new-note");
    let sb = sidebar_frame(s, session, k).label("Notes").header(new_note);
    let sb = match &s.notes.notes {
        Load::Loading => sb.child(row().key("notes-loading").w_full().py(16.0).center().child(Spinner::new().build(k))),
        Load::Failed(e) => sb.child(
            col()
                .key("notes-failed")
                .w_full()
                .padding(12.0)
                .gap(8.0)
                .child(text(e.message()).font(k.ty.small).color(k.c.text_muted).wrap())
                .child(retry(k)),
        ),
        Load::Ready(notes) => {
            let (pinned, rest): (Vec<&Note>, Vec<&Note>) = notes.iter().partition(|n| n.pinned);
            let mut sb = sb;
            if !pinned.is_empty() {
                let rows: Vec<El> = pinned.iter().map(|n| note_row(s, n, k, layer)).collect();
                sb = sb.section(SidebarSection::new("Pinned").key("pinned").rows(rows));
            }
            let rows: Vec<El> = rest.iter().map(|n| note_row(s, n, k, layer)).collect();
            sb.section(SidebarSection::new("Notes").key("notes").count(rest.len()).rows(rows))
        }
    };
    sb.build(k)
}

fn note_row(s: &App, n: &Note, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = n.id.clone();
    // The open note's row follows the editor as you type.
    let live = s.notes.open.as_ref() == Some(&n.id);
    let (title, body) = if live { (s.note_title.text.as_str(), s.note_body.text.as_str()) } else { (n.title.as_str(), n.body.as_str()) };
    let built = SidebarRow::new(if title.trim().is_empty() { "Untitled" } else { title.trim() })
        .subtitle(preview(body))
        .avatar(rune::icon(NOTE_ICON, 16.0).color(k.c.icon_muted))
        .selected(live)
        .on_select({
            let id = id.clone();
            move |s: &mut App, _: &mut Tx| s.open_note(id.clone())
        })
        .build(k)
        .key(n.id.clone())
        .on_secondary_press({
            let id = id.clone();
            move |s: &mut App, p, _: &mut Tx| s.menu = Some((MenuTarget::Note(id.clone()), p))
        });
    let point = match &s.menu {
        Some((MenuTarget::Note(m), p)) if *m == n.id => Some(*p),
        _ => None,
    };
    let pinned = n.pinned;
    context_menu(format!("note-menu-{id}"), built, point)
        .item(MenuItem::new(if pinned { "Unpin" } else { "Pin to top" }).on_select({
            let id = id.clone();
            move |s: &mut App| {
                s.menu = None;
                s.note(Event::SetPinned { id: id.clone(), pinned: !pinned });
            }
        }))
        .item(MenuItem::new("Delete note").danger(true).on_select(move |s: &mut App| {
            s.menu = None;
            s.note(Event::Delete(id.clone()));
        }))
        .on_close(|s: &mut App| s.menu = None)
        .build(k, layer)
        .key(n.id.clone())
}

fn retry(k: &Kit) -> El {
    Button::new("Try again").on_press(|s: &mut App| s.note(Event::Reload)).build(k).key("retry")
}

// The editor.

fn main_pane(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let body = match (&s.notes.notes, s.notes.open_note()) {
        (Load::Loading, _) => centered(Spinner::new().build(k)),
        (Load::Failed(e), _) => empty_state("Couldn't load your notes", e.message(), Some(retry(k)), k),
        (Load::Ready(_), Some(note)) => editor(s, note, k, layer),
        (Load::Ready(_), None) => empty_state(
            "No notes yet",
            "Write down ideas, plans and checklists. Markdown works.",
            Some(
                Button::new("New note")
                    .variant(ButtonVariant::Submit)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_note(tx))
                    .build(k)
                    .key("first-note"),
            ),
            k,
        ),
    };
    let mut pane = col().key("main").w_full().h_full().min_h(0.0);
    if let Some(n) = &s.notes.notice {
        pane = pane.child(crate::todos_ui::notice_bar(n, |s: &mut App| s.note(Event::DismissNotice), k));
    }
    pane.child(body)
}

fn editor(s: &App, note: &Note, k: &Kit, layer: &mut Vec<El>) -> El {
    let owner = (crate::api::Parent::Note, note.id.clone());
    let title = TextInput::new(&s.note_title)
        .plain()
        .text_style(TextStyle::new(26.0, 700.0).lh(34.0))
        .placeholder("Untitled")
        .width(None)
        .on_edit(|s: &mut App| &mut s.note_title)
        .on_change(|s: &mut App, tx: &mut Tx| s.edited_task(tx))
        .on_submit(|s: &mut App, tx: &mut Tx| {
            s.note_preview = false;
            tx.focus("note-body", false);
        })
        .build(k)
        .key("note-title")
        // A plain field keeps its inner padding: line the text up with the rest of the page.
        .ml(-8.0);
    let status = if s.note_edited() { "Editing…" } else { "Saved" };
    let meta = match note.updated_at.get(..10) {
        Some(day) => format!("Edited {day} · {status}"),
        None => status.to_string(),
    };
    let content = if s.note_preview {
        if s.note_body.text.trim().is_empty() {
            text("Nothing to preview yet. Switch to Write to add some text.").font(k.ty.ui).color(k.c.text_faint).key("note-empty")
        } else {
            Markdown::new(s.note_body.text.clone())
                .on_task(|s: &mut App, _: &str, offset: usize, checked: bool| s.toggle_note_task(offset, checked))
                .selectable(true)
                .build(k)
                .key("note-preview")
        }
    } else {
        TextArea::new(&s.note_body)
            .placeholder("Start writing. Markdown works: # headings, - lists, - [ ] checklists, **bold**.")
            .width(None)
            .auto_grow(16, 2000)
            .on_edit(|s: &mut App| &mut s.note_body)
            .on_change(|s: &mut App, tx: &mut Tx| s.edited_task(tx))
            .build(k)
            .key("note-body")
    };
    let page = col()
        .key("note-page")
        .w_full()
        .max_w(760.0)
        .gap(8.0)
        .child(title)
        .child(text(meta).key("note-meta").font(k.ty.small).color(k.c.text_faint).mb(12.0))
        .child(content)
        .child(el().key("files-rule").w_full().h(1.0).fill(k.c.border_weak).mt(24.0).mb(8.0))
        .child(crate::attachments_ui::section(s, &owner, k, layer));
    let scroll = col()
        .key("note-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().key("note-pad").w_full().px(40.0).pt(24.0).pb(64.0).items_center().child(page));
    crate::attachments_ui::drop_target(scroll, &owner)
}
