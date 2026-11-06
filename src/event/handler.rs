use crate::app::{ActivePane, App};
use crate::backend::BackendEvent;
use crate::event::AppEvent;
use crate::mode::AppMode;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

pub async fn handle_event(app: &mut App, event: AppEvent) -> Result<()> {
    match event {
        AppEvent::Backend(evt) => handle_backend_event(app, evt).await?,
        AppEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
            app.status_msg = None;
            handle_key_event(app, key).await?;
        }
        AppEvent::Input(Event::Paste(text)) => match app.mode {
            AppMode::Insert => app.input_buffer.push_str(&text),
            AppMode::FuzzyFind => {
                app.search_query.push_str(text.lines().next().unwrap_or(""));
                app.fuzzy_selected_idx = 0;
            }
            _ => {}
        },
        AppEvent::Input(Event::Mouse(mouse)) if app.mode == AppMode::Normal => match mouse.kind {
            MouseEventKind::ScrollDown => move_down(app).await?,
            MouseEventKind::ScrollUp => move_up(app).await?,
            _ => {}
        },
        AppEvent::Input(_) => {}
    }
    Ok(())
}

async fn handle_backend_event(app: &mut App, evt: BackendEvent) -> Result<()> {
    match evt {
        BackendEvent::NewMessage(msg) => {
            app.add_message(msg);
            app.ensure_previews().await;
        }
        BackendEvent::MessageUpdated { old_id, message } => {
            app.update_message(old_id, message);
            app.ensure_previews().await;
        }
        BackendEvent::MessagesDeleted { chat_id, message_ids } => app.remove_messages(chat_id, &message_ids),
        BackendEvent::OutboxRead { chat_id, message_id } => app.mark_outbox_read(chat_id, message_id),
        BackendEvent::History { chat_id, messages } => {
            app.apply_history(chat_id, messages);
            app.mark_active_read().await;
            app.ensure_previews().await;
        }
        BackendEvent::ChatUpdated(chat) => {
            let is_active = Some(chat.id) == app.active_chat_id();
            if app.upsert_chat(chat) {
                app.load_active_chat().await?;
            } else if is_active {
                app.mark_active_read().await;
            }
        }
        BackendEvent::ConnectionStatus(label) => app.connection = label,
        BackendEvent::Error(e) => app.set_status(format!("Telegram error: {e}")),
        BackendEvent::Status(msg) => app.set_status(msg),
        BackendEvent::FileDownloaded { file_id, path } => app.preview_downloaded(file_id, path),
        BackendEvent::ImageDecoded { file_id, result } => app.images.decoded(file_id, result),
    }
    Ok(())
}

/// Plain key press (Shift is allowed: `G`, `?`, uppercase letters...).
fn plain(key: &KeyEvent) -> bool {
    key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

fn ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

async fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<()> {
    if ctrl(&key, 'c') && app.mode == AppMode::Normal {
        app.should_quit = true;
        return Ok(());
    }
    match app.mode {
        AppMode::Normal => handle_normal_keys(app, key).await?,
        AppMode::Insert => handle_insert_keys(app, key).await?,
        AppMode::FuzzyFind => handle_fuzzy_keys(app, key).await?,
        AppMode::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
                app.mode = AppMode::Normal;
            }
        }
        AppMode::ConfirmDelete => {
            app.mode = AppMode::Normal;
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                app.delete_selected().await;
            } else {
                app.set_status("Delete cancelled");
            }
        }
    }
    Ok(())
}

async fn move_down(app: &mut App) -> Result<()> {
    match app.active_pane {
        ActivePane::Chats => {
            if let Some(idx) = app.move_chat_selection(1) {
                app.switch_chat(idx).await?;
            }
        }
        ActivePane::Messages => app.move_msg_selection(1),
    }
    Ok(())
}

async fn move_up(app: &mut App) -> Result<()> {
    match app.active_pane {
        ActivePane::Chats => {
            if let Some(idx) = app.move_chat_selection(-1) {
                app.switch_chat(idx).await?;
            }
        }
        ActivePane::Messages => {
            if app.selected_msg_idx == 0 {
                app.load_older().await?;
            } else {
                app.move_msg_selection(-1);
            }
        }
    }
    Ok(())
}

fn open_fuzzy(app: &mut App) {
    app.mode = AppMode::FuzzyFind;
    app.search_query.clear();
    app.fuzzy_selected_idx = 0;
    app.fuzzy_scroll.set(0);
}

async fn handle_normal_keys(app: &mut App, key: KeyEvent) -> Result<()> {
    // Second key of a two-key sequence.
    if let Some(first) = app.pending_key.take() {
        match (first, key.code) {
            ('g', KeyCode::Char('g')) => {
                app.selected_msg_idx = 0;
                app.load_older().await?;
            }
            (' ', KeyCode::Char('f')) => open_fuzzy(app),
            _ => {}
        }
        return Ok(());
    }

    if !plain(&key) {
        return Ok(());
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => move_down(app).await?,
        KeyCode::Char('k') | KeyCode::Up => move_up(app).await?,
        KeyCode::Char('h') | KeyCode::Left => app.active_pane = ActivePane::Chats,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => app.focus_messages().await,
        KeyCode::Char('G') | KeyCode::End => {
            app.selected_msg_idx = app.messages.len().saturating_sub(1);
        }
        KeyCode::Char('g') => app.pending_key = Some('g'),
        KeyCode::Char(' ') => app.pending_key = Some(' '),
        KeyCode::Char('r') => app.mount_reply_to_selected(),
        KeyCode::Char('y') => app.copy_selected_to_clipboard(),
        KeyCode::Char('o') => app.open_selected_media(),
        KeyCode::Char('d') => {
            if app.messages.get(app.selected_msg_idx).is_some() {
                app.mode = AppMode::ConfirmDelete;
            }
        }
        KeyCode::Char('i') | KeyCode::Char('a') => app.enter_insert(),
        KeyCode::Char('e') => app.editor_requested = true,
        KeyCode::Char('/') => open_fuzzy(app),
        KeyCode::Char('?') => app.mode = AppMode::Help,
        _ => {}
    }
    Ok(())
}

async fn handle_insert_keys(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('e') => app.editor_requested = true,
            KeyCode::Char('u') => app.input_buffer.clear(),
            KeyCode::Char('w') | KeyCode::Backspace => app.delete_word_backwards(),
            KeyCode::Char('v') => app.paste_from_clipboard(),
            KeyCode::Char('c') => app.cancel_insert(),
            _ => {}
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.cancel_insert(),
        KeyCode::Enter => app.send_current_message().await?,
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => app.delete_word_backwards(),
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Char(c) if plain(&key) => app.input_buffer.push(c),
        _ => {}
    }
    Ok(())
}

async fn handle_fuzzy_keys(app: &mut App, key: KeyEvent) -> Result<()> {
    let filtered = app.filtered_chat_indices();
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => app.mode = AppMode::Normal,
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
            if let Some(&idx) = filtered.get(app.fuzzy_selected_idx) {
                app.switch_chat(idx).await?;
                app.focus_messages().await;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if app.fuzzy_selected_idx + 1 < filtered.len() {
                app.fuzzy_selected_idx += 1;
            }
        }
        KeyCode::Char('j') | KeyCode::Char('n') if is_ctrl => {
            if app.fuzzy_selected_idx + 1 < filtered.len() {
                app.fuzzy_selected_idx += 1;
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            app.fuzzy_selected_idx = app.fuzzy_selected_idx.saturating_sub(1);
        }
        KeyCode::Char('k') | KeyCode::Char('p') if is_ctrl => {
            app.fuzzy_selected_idx = app.fuzzy_selected_idx.saturating_sub(1);
        }
        KeyCode::Char('u') if is_ctrl => {
            app.search_query.clear();
            app.fuzzy_selected_idx = 0;
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            app.fuzzy_selected_idx = 0;
        }
        KeyCode::Char(c) if plain(&key) => {
            app.search_query.push(c);
            app.fuzzy_selected_idx = 0;
        }
        _ => {}
    }
    Ok(())
}
