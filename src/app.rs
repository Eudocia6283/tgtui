use crate::backend::model::{Attachment, Chat, MediaKind, Message};
use crate::backend::{BackendEvent, TelegramBackend};
use crate::config::Config;
use crate::images::{self, ImageStore, Need};
use crate::mode::AppMode;
use crate::ui::theme::Theme;
use anyhow::{anyhow, Context, Result};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

const HISTORY_LIMIT: usize = 50;

enum Pasted {
    Text(String),
    Files(Vec<Attachment>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Chats,
    Messages,
}

pub struct App {
    pub mode: AppMode,
    pub active_pane: ActivePane,
    pub chats: Vec<Chat>,
    pub messages: Vec<Message>,
    pub selected_chat_idx: usize,
    pub selected_msg_idx: usize,
    pub replying_to: Option<String>,
    pub reply_target_id: Option<i64>,
    pub input_buffer: String,
    /// Files pasted with <C-v>, sent together with the input on <Enter>.
    pub attachments: Vec<Attachment>,
    pub search_query: String,
    pub fuzzy_selected_idx: usize,
    pub connection: String,
    /// One-shot message shown in the status bar until the next key press.
    pub status_msg: Option<String>,
    /// First key of a multi-key normal-mode command (`gg`, `<Space>f`).
    pub pending_key: Option<char>,
    /// Set by key handlers; the main loop suspends the TUI and runs $EDITOR.
    pub editor_requested: bool,
    loading_older: bool,
    // Scroll offsets persisted across frames (render only gets `&App`).
    pub msg_scroll: Cell<usize>,
    pub chat_scroll: Cell<usize>,
    pub fuzzy_scroll: Cell<usize>,
    pub images: ImageStore,
    pub backend: Arc<dyn TelegramBackend>,
    /// Lets background tasks report back (same channel as backend events).
    events_tx: mpsc::UnboundedSender<BackendEvent>,
    pub config: Config,
    pub theme: Theme,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        backend: Arc<dyn TelegramBackend>,
        config: Config,
        events_tx: mpsc::UnboundedSender<BackendEvent>,
    ) -> Self {
        Self {
            mode: AppMode::Normal,
            active_pane: ActivePane::Messages,
            chats: Vec::new(),
            messages: Vec::new(),
            selected_chat_idx: 0,
            selected_msg_idx: 0,
            replying_to: None,
            reply_target_id: None,
            input_buffer: String::new(),
            attachments: Vec::new(),
            search_query: String::new(),
            fuzzy_selected_idx: 0,
            connection: "connecting…".into(),
            status_msg: None,
            pending_key: None,
            editor_requested: false,
            loading_older: false,
            msg_scroll: Cell::new(usize::MAX),
            chat_scroll: Cell::new(0),
            fuzzy_scroll: Cell::new(0),
            theme: Theme::new(&config.ui),
            images: ImageStore::new(config.ui.inline_images && crate::ui::kitty::is_supported()),
            backend,
            events_tx,
            config,
            should_quit: false,
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        self.chats = self.backend.get_chats().await?;
        self.chats.sort_by_key(|c| std::cmp::Reverse(c.order));
        self.load_active_chat().await
    }

    pub fn active_chat(&self) -> Option<&Chat> {
        self.chats.get(self.selected_chat_idx)
    }

    pub fn active_chat_id(&self) -> Option<i64> {
        self.active_chat().map(|c| c.id)
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
    }

    pub async fn load_active_chat(&mut self) -> Result<()> {
        self.loading_older = false;
        self.msg_scroll.set(usize::MAX);
        self.messages = match self.active_chat_id() {
            Some(id) => self.backend.get_messages(id, HISTORY_LIMIT).await?,
            None => Vec::new(),
        };
        self.selected_msg_idx = self.messages.len().saturating_sub(1);
        self.mark_active_read().await;
        self.ensure_previews().await;
        Ok(())
    }

    /// Start downloading/decoding inline previews for the loaded messages.
    pub async fn ensure_previews(&mut self) {
        if !self.images.enabled {
            return;
        }
        let previews: Vec<_> = self
            .messages
            .iter()
            .filter_map(|m| m.media.as_ref()?.preview.clone())
            .collect();
        for p in previews {
            let path = p.local_path.filter(|p| p.is_file());
            match self.images.want(p.file_id, path.is_some()) {
                Need::Nothing => {}
                Need::Download => {
                    if let Err(e) = self.backend.download_file(p.file_id).await {
                        self.images.decoded(p.file_id, Err(e.to_string()));
                    }
                }
                Need::Decode => self.decode_preview(p.file_id, path.unwrap_or_default()),
            }
        }
    }

    pub fn preview_downloaded(&mut self, file_id: i32, path: PathBuf) {
        if self.images.downloaded(file_id) {
            self.decode_preview(file_id, path);
        }
    }

    fn decode_preview(&self, file_id: i32, path: PathBuf) {
        let tx = self.events_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = images::decode(&path);
            let _ = tx.send(BackendEvent::ImageDecoded { file_id, result });
        });
    }

    /// Select chat `idx` and load its history.
    pub async fn switch_chat(&mut self, idx: usize) -> Result<()> {
        if idx >= self.chats.len() || idx == self.selected_chat_idx && !self.messages.is_empty() {
            return Ok(());
        }
        self.selected_chat_idx = idx;
        self.replying_to = None;
        self.reply_target_id = None;
        self.load_active_chat().await
    }

    pub async fn focus_messages(&mut self) {
        self.active_pane = ActivePane::Messages;
        self.mark_active_read().await;
    }

    /// Mark the open chat as read if the user is actually looking at it.
    pub async fn mark_active_read(&mut self) {
        if !self.config.general.auto_mark_read || self.active_pane != ActivePane::Messages {
            return;
        }
        let Some(last_id) = self.messages.last().map(|m| m.id) else {
            return;
        };
        let idx = self.selected_chat_idx;
        if let Some(chat) = self.chats.get_mut(idx).filter(|c| c.unread_count > 0) {
            chat.unread_count = 0;
            let chat_id = chat.id;
            if let Err(e) = self.backend.mark_as_read(chat_id, last_id).await {
                self.set_status(format!("Failed to mark as read: {e}"));
            }
        }
    }

    /// Insert/update/remove a chat keeping the list sorted and the current
    /// selection stable. Returns true if the active chat changed and needs loading.
    pub fn upsert_chat(&mut self, chat: Chat) -> bool {
        let active = self.active_chat_id();
        match self.chats.iter().position(|c| c.id == chat.id) {
            Some(i) if chat.order == 0 => {
                self.chats.remove(i);
            }
            Some(i) => self.chats[i] = chat,
            None if chat.order != 0 => self.chats.push(chat),
            None => return false,
        }
        self.chats.sort_by_key(|c| std::cmp::Reverse(c.order));
        match active.and_then(|id| self.chats.iter().position(|c| c.id == id)) {
            Some(i) => {
                self.selected_chat_idx = i;
                false
            }
            None => {
                self.selected_chat_idx = 0;
                true
            }
        }
    }

    fn select_message_id_or_clamp(&mut self, keep_bottom: bool, selected_id: Option<i64>) {
        let last = self.messages.len().saturating_sub(1);
        self.selected_msg_idx = if keep_bottom {
            last
        } else {
            selected_id
                .and_then(|id| self.messages.iter().position(|m| m.id == id))
                .unwrap_or(self.selected_msg_idx)
                .min(last)
        };
    }

    fn is_at_bottom(&self) -> bool {
        self.selected_msg_idx + 1 >= self.messages.len()
    }

    pub fn apply_history(&mut self, chat_id: i64, messages: Vec<Message>) {
        if Some(chat_id) != self.active_chat_id() {
            return;
        }
        let keep_bottom = self.is_at_bottom();
        let selected_id = self.messages.get(self.selected_msg_idx).map(|m| m.id);
        self.messages = messages;
        self.loading_older = false;
        self.select_message_id_or_clamp(keep_bottom, selected_id);
    }

    pub fn add_message(&mut self, msg: Message) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == msg.chat_id) {
            chat.last_message_preview = msg.text.replace('\n', " ");
            chat.last_message_time = msg.time_str.clone();
        }
        if Some(msg.chat_id) != self.active_chat_id() {
            self.notify(&msg);
            return;
        }
        if self.messages.iter().any(|m| m.id == msg.id) {
            return;
        }
        let keep_bottom = self.is_at_bottom() || msg.is_outgoing;
        let selected_id = self.messages.get(self.selected_msg_idx).map(|m| m.id);
        let pos = self.messages.partition_point(|m| m.id < msg.id);
        self.messages.insert(pos, msg);
        self.select_message_id_or_clamp(keep_bottom, selected_id);
    }

    pub fn update_message(&mut self, old_id: i64, msg: Message) {
        if Some(msg.chat_id) != self.active_chat_id() {
            return;
        }
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == old_id) {
            *m = msg;
        }
        if self.reply_target_id == Some(old_id) {
            self.reply_target_id = self.messages.get(self.selected_msg_idx).map(|m| m.id);
        }
    }

    pub fn remove_messages(&mut self, chat_id: i64, ids: &[i64]) {
        if Some(chat_id) != self.active_chat_id() {
            return;
        }
        self.messages.retain(|m| !ids.contains(&m.id));
        self.selected_msg_idx = self.selected_msg_idx.min(self.messages.len().saturating_sub(1));
    }

    pub fn mark_outbox_read(&mut self, chat_id: i64, up_to: i64) {
        if Some(chat_id) != self.active_chat_id() {
            return;
        }
        for m in self.messages.iter_mut().filter(|m| m.is_outgoing && m.id <= up_to) {
            m.is_read = true;
        }
    }

    pub fn move_chat_selection(&mut self, delta: isize) -> Option<usize> {
        let target = self.selected_chat_idx.checked_add_signed(delta)?;
        (target < self.chats.len()).then_some(target)
    }

    pub fn move_msg_selection(&mut self, delta: isize) {
        let last = self.messages.len().saturating_sub(1);
        self.selected_msg_idx = self.selected_msg_idx.saturating_add_signed(delta).min(last);
    }

    /// Fetch older history when the user scrolls past the top.
    pub async fn load_older(&mut self) -> Result<()> {
        if self.loading_older {
            return Ok(());
        }
        if let (Some(chat_id), Some(first)) = (self.active_chat_id(), self.messages.first()) {
            self.loading_older = true;
            self.backend.load_older(chat_id, first.id).await?;
        }
        Ok(())
    }

    /// Indices into `self.chats` matching the search query, best match first.
    pub fn filtered_chat_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return (0..self.chats.len()).collect();
        }
        let matcher = SkimMatcherV2::default().ignore_case();
        let mut scored: Vec<(i64, usize)> = self
            .chats
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let title = matcher.fuzzy_match(&c.title, &self.search_query);
                let user = c
                    .username
                    .as_deref()
                    .and_then(|u| matcher.fuzzy_match(u, &self.search_query));
                title.max(user).map(|score| (score, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    pub async fn send_current_message(&mut self) -> Result<()> {
        let text = self.input_buffer.trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return Ok(());
        }
        let Some(chat_id) = self.active_chat_id() else {
            self.set_status("No chat selected");
            return Ok(());
        };
        // The backend broadcasts BackendEvent::NewMessage; don't push here.
        let result = if self.attachments.is_empty() {
            self.backend.send_message(chat_id, text, self.reply_target_id).await
        } else {
            // The text becomes the caption of the first file.
            let mut caption = text;
            let mut result = Ok(());
            for file in std::mem::take(&mut self.attachments) {
                let reply_to = self.reply_target_id;
                result = self.backend.send_file(chat_id, file, std::mem::take(&mut caption), reply_to).await;
                if result.is_err() {
                    break;
                }
            }
            result
        };
        if let Err(e) = result {
            self.set_status(format!("Send failed: {e}"));
            return Ok(());
        }

        self.input_buffer.clear();
        self.replying_to = None;
        self.reply_target_id = None;
        self.mode = AppMode::Normal;
        Ok(())
    }

    pub fn enter_insert(&mut self) {
        // In single-column mode the input box lives in the messages pane.
        self.active_pane = ActivePane::Messages;
        self.mode = AppMode::Insert;
    }

    pub fn mount_reply_to_selected(&mut self) {
        if let Some(msg) = self.messages.get(self.selected_msg_idx) {
            let flat = msg.text.replace('\n', " ");
            let preview = if flat.chars().count() > 30 {
                format!("{}...", flat.chars().take(28).collect::<String>())
            } else {
                flat
            };
            self.replying_to = Some(format!("{}: {}", msg.sender_name, preview));
            self.reply_target_id = Some(msg.id);
            self.enter_insert();
        }
    }

    /// Leave INSERT mode, dropping any mounted reply and attachments.
    pub fn cancel_insert(&mut self) {
        self.mode = AppMode::Normal;
        self.replying_to = None;
        self.reply_target_id = None;
        self.attachments.clear();
    }

    pub async fn delete_selected(&mut self) {
        let Some((chat_id, msg_id)) = self
            .messages
            .get(self.selected_msg_idx)
            .map(|m| (m.chat_id, m.id))
        else {
            return;
        };
        match self.backend.delete_message(chat_id, msg_id).await {
            Ok(()) => self.set_status("Message deleted"),
            Err(e) => self.set_status(format!("Delete failed: {e}")),
        }
    }

    /// `y`: copy the selected message's media (image data, or the file as a
    /// `text/uri-list` like file managers do); plain text otherwise.
    pub fn copy_selected_to_clipboard(&mut self) {
        let Some(msg) = self.messages.get(self.selected_msg_idx) else {
            return;
        };
        let Some(media) = msg.media.clone() else {
            self.copy_selected_text();
            return;
        };
        let backend = self.backend.clone();
        let tx = self.events_tx.clone();
        let copy_cmd = self.config.wayland.copy_command.clone();
        self.set_status("Downloading media to yank…");
        tokio::spawn(async move {
            let status = match backend.download_media(media.file_id).await {
                Ok(path) => {
                    let payload =
                        tokio::task::spawn_blocking(move || clipboard_payload(&path, media.kind)).await;
                    match payload {
                        Ok(Ok(p)) => match copy_to_clipboard(&copy_cmd, p.mime, p.data) {
                            Ok(()) => format!("Yanked {} to clipboard", p.what),
                            Err(e) => format!("Copy failed ({copy_cmd}): {e:#}"),
                        },
                        Ok(Err(e)) => format!("Copy failed: {e:#}"),
                        Err(e) => format!("Copy failed: {e}"),
                    }
                }
                Err(e) => format!("Media download failed: {e}"),
            };
            let _ = tx.send(BackendEvent::Status(status));
        });
    }

    /// `Y`: copy only the text (for media messages: the caption).
    pub fn copy_selected_text(&mut self) {
        let Some(msg) = self.messages.get(self.selected_msg_idx) else {
            return;
        };
        let text = if msg.media.is_some() { strip_media_label(&msg.text) } else { msg.text.as_str() }.to_string();
        if text.is_empty() {
            self.set_status("Nothing to yank: message has no text");
            return;
        }
        let cmd = self.config.wayland.copy_command.clone();
        match copy_to_clipboard(&cmd, "text/plain;charset=utf-8", text.into_bytes()) {
            Ok(()) => self.set_status("Yanked text to clipboard"),
            Err(e) => self.set_status(format!("Copy failed ({cmd}): {e:#}")),
        }
    }

    /// Paste from the clipboard (wl-paste): images and copied files become
    /// attachments, anything else is inserted as text.
    pub fn paste_from_clipboard(&mut self) {
        match self.read_clipboard() {
            Ok(Pasted::Text(text)) => self.input_buffer.push_str(&text),
            Ok(Pasted::Files(files)) => {
                let n = files.len();
                self.attachments.extend(files);
                self.set_status(format!("Attached {n} file(s) — <Enter> to send, <Esc> to cancel"));
            }
            Err(e) => self.set_status(format!("Paste failed: {e:#}")),
        }
    }

    fn read_clipboard(&self) -> Result<Pasted> {
        let paste = &self.config.wayland.paste_command;
        let types = run_capture(paste, &["--list-types"]).unwrap_or_default();
        let types: Vec<&str> = std::str::from_utf8(&types).unwrap_or("").lines().map(str::trim).collect();

        // Files copied in a file manager.
        if types.contains(&"text/uri-list") {
            let list = run_capture(paste, &["--type", "text/uri-list"])?;
            let files: Vec<Attachment> = String::from_utf8_lossy(&list)
                .lines()
                .filter_map(|l| l.trim().strip_prefix("file://").map(percent_decode))
                .map(PathBuf::from)
                .filter(|p| p.is_file())
                .map(|path| Attachment { kind: media_kind_for(&path), path })
                .collect();
            if !files.is_empty() {
                return Ok(Pasted::Files(files));
            }
        }

        // Raw image data (screenshots, "copy image").
        const IMAGE_TYPES: [(&str, &str); 4] = [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/webp", "webp"),
            ("image/gif", "gif"),
        ];
        if let Some((mime, ext)) = IMAGE_TYPES.iter().find(|(m, _)| types.contains(m)) {
            let data = run_capture(paste, &["--type", mime])?;
            let path = paste_cache_file(ext)?;
            std::fs::write(&path, data)?;
            // GIFs are sent as documents so they keep their animation.
            let kind = if *ext == "gif" { MediaKind::File } else { MediaKind::Image };
            return Ok(Pasted::Files(vec![Attachment { path, kind }]));
        }

        let out = run_capture(paste, &["--no-newline"]).or_else(|_| run_capture(paste, &[]))?;
        let text = String::from_utf8(out).map_err(|_| anyhow!("clipboard does not contain text"))?;
        Ok(Pasted::Text(text.strip_suffix('\n').unwrap_or(&text).to_string()))
    }

    /// Download (if needed) and open the selected message's media in the
    /// background; the result is reported through the status bar.
    pub fn open_selected_media(&mut self) {
        let Some(media) = self.messages.get(self.selected_msg_idx).and_then(|m| m.media.clone()) else {
            self.set_status("Selected message has no media");
            return;
        };
        let opener = self.config.wayland.open_command.clone();
        let cmd = match media.kind {
            MediaKind::Image => self.config.wayland.image_viewer.clone(),
            MediaKind::File => opener.clone(),
        };
        let backend = self.backend.clone();
        let tx = self.events_tx.clone();
        self.set_status("Downloading media…");
        tokio::spawn(async move {
            let msg = match backend.download_media(media.file_id).await {
                Ok(path) => {
                    let arg = [path.to_string_lossy().into_owned()];
                    // Fall back to the generic opener if the image viewer isn't installed.
                    let used = match spawn_command(&cmd, &arg, None) {
                        Ok(()) => Ok(cmd),
                        Err(_) if cmd != opener => spawn_command(&opener, &arg, None).map(|_| opener),
                        Err(e) => Err(e),
                    };
                    match used {
                        Ok(used) => format!("Opened {} with {used}", path.display()),
                        Err(e) => format!("Failed to open {}: {e}", path.display()),
                    }
                }
                Err(e) => format!("Media download failed: {e}"),
            };
            let _ = tx.send(BackendEvent::Status(msg));
        });
    }

    pub fn delete_word_backwards(&mut self) {
        let trimmed = self.input_buffer.trim_end().len();
        self.input_buffer.truncate(trimmed);
        let cut = self
            .input_buffer
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.input_buffer.truncate(cut);
    }

    fn notify(&self, msg: &Message) {
        if msg.is_outgoing || self.config.wayland.notify_command.trim().is_empty() {
            return;
        }
        // Skip backlog delivered right after (re)connecting.
        if chrono::Utc::now().timestamp() - msg.date > 60 {
            return;
        }
        let Some(chat) = self.chats.iter().find(|c| c.id == msg.chat_id) else {
            return;
        };
        if chat.is_muted {
            return;
        }
        let title = chat.title.clone();
        let body = format!("{}: {}", msg.sender_name, msg.text);
        let _ = spawn_command(&self.config.wayland.notify_command, &[title, body], None);
    }

    /// Run the external editor on the current input. Must be called with the
    /// TUI suspended. Returns the edited text if the editor exited cleanly.
    pub fn run_editor(&self) -> Result<Option<String>> {
        use std::os::unix::fs::OpenOptionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("telegram-tui-{}-{}.md", std::process::id(), nanos));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(self.input_buffer.as_bytes())?;
        drop(file);

        let (program, args) = split_command(&self.config.general.editor)?;
        let status = Command::new(program)
            .args(args)
            .arg(&path)
            .status()
            .with_context(|| format!("failed to launch editor `{program}`"));
        let content = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);

        if !status?.success() {
            return Ok(None);
        }
        let trimmed = content?.trim().to_string();
        Ok((!trimmed.is_empty()).then_some(trimmed))
    }
}

fn split_command(line: &str) -> Result<(&str, std::str::SplitWhitespace<'_>)> {
    let mut parts = line.split_whitespace();
    let program = parts.next().ok_or_else(|| anyhow!("empty command"))?;
    Ok((program, parts))
}

/// Spawn a helper without touching the TUI's stdout/stderr, optionally feeding
/// `stdin`, and reap it in the background so it doesn't linger as a zombie.
fn spawn_command(line: &str, extra_args: &[String], stdin: Option<Vec<u8>>) -> Result<()> {
    let (program, args) = split_command(line)?;
    let mut child = Command::new(program)
        .args(args)
        .args(extra_args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let _ = pipe.write_all(&data);
        }
        let _ = child.wait();
    });
    Ok(())
}

/// "[Photo] caption" -> "caption".
pub fn strip_media_label(text: &str) -> &str {
    match text.strip_prefix('[').and_then(|t| t.split_once(']')) {
        Some((_, rest)) => rest.trim_start(),
        None => text,
    }
}

struct ClipboardPayload {
    mime: &'static str,
    data: Vec<u8>,
    what: &'static str,
}

/// Images become PNG data (what most apps accept on paste); other files a
/// `text/uri-list`, so they paste into file managers or back into a chat.
fn clipboard_payload(path: &Path, kind: MediaKind) -> Result<ClipboardPayload> {
    if kind == MediaKind::Image {
        if let Ok(img) = image::open(path) {
            let mut data = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png)?;
            return Ok(ClipboardPayload { mime: "image/png", data, what: "image" });
        }
    }
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let uri = format!("file://{}\r\n", percent_encode(&path.to_string_lossy()));
    Ok(ClipboardPayload { mime: "text/uri-list", data: uri.into_bytes(), what: "file" })
}

/// Pipe `data` into the copy command, telling it the MIME type when we
/// know how (wl-copy / xclip); other commands just get the bytes.
fn copy_to_clipboard(line: &str, mime: &str, data: Vec<u8>) -> Result<()> {
    let (program, _) = split_command(line)?;
    let name = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program);
    let type_args: Vec<String> = match name {
        "wl-copy" => vec!["--type".into(), mime.into()],
        "xclip" => vec!["-t".into(), mime.into()],
        _ => Vec::new(),
    };
    spawn_command(line, &type_args, Some(data))
}

/// Percent-encode a path for a file:// URI (keeps `/` and unreserved bytes).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Run a helper command and capture its stdout (fails on non-zero exit).
fn run_capture(line: &str, extra_args: &[&str]) -> Result<Vec<u8>> {
    let (program, args) = split_command(line)?;
    let out = Command::new(program)
        .args(args)
        .args(extra_args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("failed to run `{program}`"))?;
    if !out.status.success() {
        return Err(anyhow!("clipboard is empty"));
    }
    Ok(out.stdout)
}

fn media_kind_for(path: &Path) -> MediaKind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" => MediaKind::Image,
        _ => MediaKind::File,
    }
}

/// Decode `%XX` escapes in a file:// URI path.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        if bytes[i] == b'%' {
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A fresh file in ~/.cache/telegram-tui/ for pasted image data. The file must
/// outlive the (asynchronous) upload, so old ones are pruned after a day instead.
fn paste_cache_file(ext: &str) -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("telegram-tui");
    std::fs::create_dir_all(&dir)?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
        for e in entries.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < day_ago);
            if old && e.file_name().to_string_lossy().starts_with("paste-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("paste-{ts}.{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/home/u/My%20File.png"), "/home/u/My File.png");
        assert_eq!(percent_decode("/tmp/%E4%BD%A0%E5%A5%BD.txt"), "/tmp/你好.txt");
        assert_eq!(percent_decode("/tmp/100%"), "/tmp/100%");
        assert_eq!(percent_decode("/tmp/%zz"), "/tmp/%zz");
    }

    async fn app_with_paste(script: &str) -> (App, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        let bin = dir.0.join("fake-paste");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut config = Config::default();
        config.wayland.paste_command = bin.to_string_lossy().into_owned();
        let backend = Arc::new(crate::backend::mock::MockBackend::new(tx.clone()));
        (App::new(backend, config, tx), dir)
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let p = std::env::temp_dir().join(format!("tgtui-test-{}-{n}", std::process::id()));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[tokio::test]
    async fn paste_image_becomes_attachment() {
        let (mut app, _d) = app_with_paste(
            r#"case "$1" in --list-types) printf 'image/png\ntext/plain\n';; --type) printf 'PNGDATA';; *) printf text;; esac"#,
        )
        .await;
        app.paste_from_clipboard();
        assert_eq!(app.attachments.len(), 1, "{:?}", app.status_msg);
        let a = &app.attachments[0];
        assert_eq!(a.kind, MediaKind::Image);
        assert_eq!(std::fs::read(&a.path).unwrap(), b"PNGDATA");
        assert!(app.input_buffer.is_empty());
        let _ = std::fs::remove_file(&a.path);
    }

    #[tokio::test]
    async fn paste_copied_files_and_text() {
        let (mut app, d) = app_with_paste("").await;
        let file = d.0.join("my report.pdf");
        std::fs::write(&file, "x").unwrap();
        let uri = format!("file://{}", file.to_string_lossy().replace(' ', "%20"));
        let script = format!(
            r#"case "$1" in --list-types) echo text/uri-list;; --type) echo '{uri}';; esac"#
        );
        std::fs::write(d.0.join("fake-paste"), format!("#!/bin/sh\n{script}\n")).unwrap();
        app.paste_from_clipboard();
        assert_eq!(app.attachments.len(), 1, "{:?}", app.status_msg);
        assert_eq!(app.attachments[0].path, file);
        assert_eq!(app.attachments[0].kind, MediaKind::File);

        std::fs::write(
            d.0.join("fake-paste"),
            "#!/bin/sh\ncase \"$1\" in --list-types) printf 'text/plain\\n';; *) printf 'hello';; esac\n",
        )
        .unwrap();
        app.paste_from_clipboard();
        assert_eq!(app.input_buffer, "hello");
    }

    #[test]
    fn percent_encoding_roundtrips() {
        let p = "/home/u/My File 你好#1.pdf";
        let enc = percent_encode(p);
        assert_eq!(enc, "/home/u/My%20File%20%E4%BD%A0%E5%A5%BD%231.pdf");
        assert_eq!(percent_decode(&enc), p);
    }

    #[test]
    fn clipboard_payload_for_image_and_file() {
        let d = tempdir::Dir::new();
        let jpg = d.0.join("photo.jpg");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3])).save(&jpg).unwrap();
        let p = clipboard_payload(&jpg, MediaKind::Image).unwrap();
        assert_eq!(p.mime, "image/png");
        let png = image::load_from_memory(&p.data).unwrap();
        assert_eq!((png.width(), png.height()), (4, 3));

        let doc = d.0.join("a b.pdf");
        std::fs::write(&doc, "x").unwrap();
        let p = clipboard_payload(&doc, MediaKind::File).unwrap();
        assert_eq!(p.mime, "text/uri-list");
        let uri = String::from_utf8(p.data).unwrap();
        assert!(uri.starts_with("file:///") && uri.ends_with("a%20b.pdf\r\n"), "{uri}");

        // An "image" that can't be decoded is still yanked, as a file.
        let bogus = d.0.join("x.webm");
        std::fs::write(&bogus, "not an image").unwrap();
        assert_eq!(clipboard_payload(&bogus, MediaKind::Image).unwrap().mime, "text/uri-list");
    }

    #[test]
    fn copy_passes_mime_type_to_wl_copy() {
        let d = tempdir::Dir::new();
        let bin = d.0.join("wl-copy");
        let log = d.0.join("log");
        std::fs::write(&bin, format!("#!/bin/sh\necho \"$@\" > {0}.args\ncat > {0}.data\n", log.display())).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        copy_to_clipboard(&bin.to_string_lossy(), "image/png", b"DATA".to_vec()).unwrap();
        let args = log.with_extension("args");
        for _ in 0..100 {
            if std::fs::read(log.with_extension("data")).is_ok_and(|d| d == b"DATA") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(args).unwrap().trim(), "--type image/png");
        assert_eq!(std::fs::read(log.with_extension("data")).unwrap(), b"DATA");
    }

    #[test]
    fn media_kind_from_extension() {
        assert_eq!(media_kind_for(Path::new("a.PNG")), MediaKind::Image);
        assert_eq!(media_kind_for(Path::new("a.jpeg")), MediaKind::Image);
        assert_eq!(media_kind_for(Path::new("a.gif")), MediaKind::File);
        assert_eq!(media_kind_for(Path::new("a.pdf")), MediaKind::File);
    }
}
