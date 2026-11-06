//! End-to-end tests: drive the real App with the mock backend and render
//! into ratatui's TestBackend.

use crate::app::{ActivePane, App};
use crate::backend::mock::MockBackend;
use crate::backend::BackendEvent;
use crate::config::Config;
use crate::event::{handler::handle_event, AppEvent};
use crate::mode::AppMode;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use tokio::sync::mpsc;

async fn setup() -> (App, mpsc::UnboundedReceiver<BackendEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App::new(Arc::new(MockBackend::new(tx.clone())), Config::default(), tx);
    app.init().await.unwrap();
    (app, rx)
}

async fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    let evt = AppEvent::Input(Event::Key(KeyEvent::new(code, mods)));
    handle_event(app, evt).await.unwrap();
}

async fn chars(app: &mut App, s: &str) {
    for c in s.chars() {
        let mods = if c.is_uppercase() || "?!G".contains(c) { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        key(app, KeyCode::Char(c), mods).await;
    }
}

/// Feed queued backend events into the app, like the main loop does.
async fn pump(app: &mut App, rx: &mut mpsc::UnboundedReceiver<BackendEvent>) {
    while let Ok(evt) = rx.try_recv() {
        handle_event(app, AppEvent::Backend(evt)).await.unwrap();
    }
}

fn screen(app: &App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn shift_keys_work_in_normal_and_insert() {
    let (mut app, mut rx) = setup().await;
    chars(&mut app, "?").await;
    assert_eq!(app.mode, AppMode::Help);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;

    // Jump to the group chat via the fuzzy finder and type with capitals.
    chars(&mut app, "/arch").await;
    assert_eq!(app.mode, AppMode::FuzzyFind);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
    assert_eq!(app.active_chat().unwrap().title, "#archlinux-general");
    assert_eq!(app.active_chat().unwrap().unread_count, 0, "opening marks read");

    chars(&mut app, "i").await;
    chars(&mut app, "Hello World!").await;
    assert_eq!(app.input_buffer, "Hello World!");
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL).await;
    assert_eq!(app.input_buffer, "Hello ");
    chars(&mut app, "TUI").await;
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
    pump(&mut app, &mut rx).await;

    assert_eq!(app.mode, AppMode::Normal);
    let last = app.messages.last().unwrap();
    assert_eq!(last.text, "Hello TUI");
    assert_eq!(app.selected_msg_idx, app.messages.len() - 1);
    assert!(screen(&app, 120, 30).contains("Hello TUI"));
}

#[tokio::test]
async fn gg_g_reply_and_delete() {
    let (mut app, mut rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    let n = app.messages.len();
    chars(&mut app, "gg").await;
    assert_eq!(app.selected_msg_idx, 0);
    chars(&mut app, "G").await;
    assert_eq!(app.selected_msg_idx, n - 1);

    chars(&mut app, "k").await;
    chars(&mut app, "r").await;
    assert_eq!(app.mode, AppMode::Insert);
    assert!(app.replying_to.as_deref().unwrap().starts_with("Bob:"));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;
    assert!(app.replying_to.is_none());

    // d + anything else cancels; d + y deletes.
    chars(&mut app, "dn").await;
    assert_eq!(app.messages.len(), n);
    chars(&mut app, "dy").await;
    pump(&mut app, &mut rx).await;
    assert_eq!(app.messages.len(), n - 1);
}

#[tokio::test]
async fn long_history_scrolls_to_latest_and_wraps() {
    let (mut app, _rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    let template = app.messages[0].clone();
    let base = app.messages.last().unwrap().id;
    for i in 1..=40 {
        let mut m = template.clone();
        m.id = base + i;
        m.text = format!("filler message number {i} with some extra words to force wrapping in a narrow pane");
        app.add_message(m);
    }
    let s = screen(&app, 100, 30);
    assert!(s.contains("number 40"), "newest message must be visible:\n{s}");
    assert!(s.contains("narrow pane"), "long text must wrap, not be cut:\n{s}");
    assert!(!s.contains("number 1 "), "oldest should be scrolled away:\n{s}");

    chars(&mut app, "gg").await;
    let s = screen(&app, 100, 30);
    assert!(s.contains("Has anyone tested"), "gg scrolls to the top:\n{s}");
}

#[tokio::test]
async fn compact_mode_and_tiny_terminal_dont_panic() {
    let (mut app, _rx) = setup().await;
    for (w, h) in [(60, 20), (20, 6), (5, 3), (1, 1)] {
        screen(&app, w, h);
    }
    chars(&mut app, "h").await;
    assert_eq!(app.active_pane, ActivePane::Chats);
    assert!(screen(&app, 60, 20).contains("Chats (6)"));
    chars(&mut app, "?").await;
    screen(&app, 30, 10);
}


#[tokio::test]
async fn own_messages_right_others_left_in_bubbles() {
    let (mut app, _rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    let s = screen(&app, 110, 30);
    let col = |needle: &str| {
        let line = s.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} missing:\n{s}"));
        // Column in cells (screen() emits one char per cell), not bytes.
        line[..line.find(needle).unwrap()].chars().count()
    };
    // Others start near the left edge of the pane, own messages are pushed right.
    assert!(col("Yep, writing") > col("Works like magic") + 10, "{s}");
    // Bubbles have borders with the sender in the top edge.
    assert!(s.contains("╭─ Bob  14:25"), "{s}");
    assert!(s.contains("╭─ 14:26 ✓✓"), "{s}");
    assert!(s.contains("┃ Alice: Has anyone"), "reply quote inside bubble:\n{s}");
}

#[tokio::test]
async fn attachments_are_sent_with_caption() {
    use crate::backend::model::{Attachment, MediaKind};
    let (mut app, mut rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    chars(&mut app, "i").await;
    app.attachments.push(Attachment { path: "/tmp/shot.png".into(), kind: MediaKind::Image });
    assert!(screen(&app, 110, 30).contains("shot.png"), "attachment shown in input box");
    chars(&mut app, "look").await;
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
    pump(&mut app, &mut rx).await;
    assert!(app.attachments.is_empty());
    assert_eq!(app.messages.last().unwrap().text, "[Photo: shot.png] look");

    // Esc drops pending attachments.
    chars(&mut app, "i").await;
    app.attachments.push(Attachment { path: "/tmp/a.pdf".into(), kind: MediaKind::File });
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;
    assert!(app.attachments.is_empty());
}

#[tokio::test]
async fn open_media_reports_status() {
    let (mut app, mut rx) = setup().await;
    // Saved Messages: the last demo message carries a photo.
    chars(&mut app, "o").await;
    assert_eq!(app.status_msg.as_deref(), Some("Downloading media…"));
    // The background task reports via the event channel.
    loop {
        let evt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        let is_status = matches!(evt, BackendEvent::Status(_));
        handle_event(&mut app, AppEvent::Backend(evt)).await.unwrap();
        if is_status {
            break;
        }
    }
    assert!(app.status_msg.unwrap().contains("demo mode"));
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_image_renders_with_kitty_placeholders() {
    use crate::backend::model::{Media, MediaKind, Preview};
    let (mut app, mut rx) = setup().await;
    app.images.enabled = true;
    app.images.cell_px.set((10, 20));

    let path = std::env::temp_dir().join(format!("tgtui-img-{}.png", std::process::id()));
    image::RgbImage::from_pixel(200, 100, image::Rgb([255, 0, 0])).save(&path).unwrap();

    app.switch_chat(1).await.unwrap();
    let mut m = app.messages[0].clone();
    m.id = 999;
    m.text = "[Photo] sunset".into();
    m.media = Some(Media {
        file_id: 50,
        kind: MediaKind::Image,
        preview: Some(Preview { file_id: 51, local_path: Some(path.clone()) }),
    });
    app.add_message(m);
    app.ensure_previews().await;

    // Decoding runs on a worker thread and reports back via the channel.
    loop {
        let evt = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        let done = matches!(evt, BackendEvent::ImageDecoded { .. });
        handle_event(&mut app, AppEvent::Backend(evt)).await.unwrap();
        if done {
            break;
        }
    }
    let _ = std::fs::remove_file(&path);
    assert!(app.images.ready(51).is_some());

    let s = screen(&app, 110, 40);
    // 200x100 px at 10x20 px cells = 20 cols x 5 rows of placeholders.
    let rows: Vec<&str> = s.lines().filter(|l| l.contains(crate::ui::kitty::PLACEHOLDER)).collect();
    assert_eq!(rows.len(), 5, "{s}");
    assert_eq!(rows[0].matches(crate::ui::kitty::PLACEHOLDER).count(), 20);
    // Inside the bubble the image replaces the "[Photo]" label (the chat list keeps it).
    assert!(s.contains("│ sunset "), "label replaced by image:\n{s}");

    let mut out = Vec::new();
    app.images.flush(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("a=t,f=100"), "image transmitted");
    assert!(out.contains("a=p,U=1,") && out.contains("c=20,r=5"), "virtual placement: {}", &out[out.len().saturating_sub(80)..]);

    // Next frame: already transmitted and placed -> nothing to send.
    screen(&app, 110, 40);
    let mut out = Vec::new();
    app.images.flush(&mut out).unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn failed_send_is_shown_in_bubble() {
    use crate::backend::model::SendState;
    let (mut app, _rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    let mut m = app.messages.last().unwrap().clone();
    let old = m.id;
    m.send_state = SendState::Failed("PHOTO_INVALID_DIMENSIONS".into());
    app.update_message(old, m);
    let s = screen(&app, 110, 30);
    assert!(s.contains("✗") && s.contains("PHOTO_INVALID_DIMENSIONS"), "{s}");
}

#[tokio::test]
async fn chat_list_has_separators_between_chats() {
    let (app, _rx) = setup().await;
    let s = screen(&app, 110, 30);
    // Left pane is 30 columns wide: a separator spans its inner width (28).
    let separators = s
        .lines()
        .filter(|l| l.starts_with('│') && l.chars().skip(1).take(28).all(|c| c == '┄'))
        .count();
    assert_eq!(separators, 5, "6 chats -> 5 separators, none after the last:\n{s}");
}

#[tokio::test]
async fn ui_uses_no_emoji() {
    let (mut app, _rx) = setup().await;
    let is_emoji = |c: char| matches!(c as u32, 0x1F000..=0x1FFFF | 0x2600..=0x26FF);
    for keys in ["", "/", "\u{1b}?"] {
        for c in keys.chars() {
            let code = if c == '\u{1b}' { KeyCode::Esc } else { KeyCode::Char(c) };
            key(&mut app, code, KeyModifiers::NONE).await;
        }
        let s = screen(&app, 120, 40);
        // Message text may contain emoji; the chrome must not.
        let found: Vec<char> = s.chars().filter(|&c| is_emoji(c)).collect();
        assert!(found.is_empty(), "emoji {found:?} after {keys:?}:\n{s}");
    }
    // Nerd Font icons are used for pin / mute (Saved Messages is pinned, Noctalia muted).
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;
    let s = screen(&app, 120, 40);
    assert!(s.contains("\u{f435}") && s.contains("\u{f1f6}"), "{s}");
}


#[tokio::test]
async fn ticks_switch_from_sent_to_read() {
    let (mut app, mut rx) = setup().await;
    app.switch_chat(1).await.unwrap();
    chars(&mut app, "iping").await;
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
    pump(&mut app, &mut rx).await;
    let last = app.messages.last().unwrap().clone();
    assert!(!last.is_read);
    let tick_line = |s: &str| s.lines().find(|l| l.contains(&format!("─ {} ", last.time_str)) && l.contains('✓')).map(str::to_string);
    let before = tick_line(&screen(&app, 110, 30)).unwrap();
    assert!(before.contains(" ✓ ") && !before.contains("✓✓"), "{before}");

    handle_event(&mut app, AppEvent::Backend(BackendEvent::OutboxRead { chat_id: last.chat_id, message_id: last.id }))
        .await
        .unwrap();
    let after = tick_line(&screen(&app, 110, 30)).unwrap();
    assert!(after.contains("✓✓"), "{after}");
}
