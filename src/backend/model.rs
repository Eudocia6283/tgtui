use chrono::{DateTime, Datelike, Local};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    Direct,
    Group,
    Channel,
    Bot,
}

#[derive(Debug, Clone)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub username: Option<String>,
    pub chat_type: ChatType,
    pub unread_count: u32,
    pub is_pinned: bool,
    pub is_muted: bool,
    pub last_message_preview: String,
    pub last_message_time: String,
    pub members_count: Option<u32>,
    /// Position in the main chat list (higher = closer to top). 0 = not in list.
    pub order: i64,
}

#[derive(Debug, Clone)]
pub struct ReplyInfo {
    pub sender_name: String,
    pub text_preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Opened with `image_viewer`.
    Image,
    /// Anything else, opened with `open_command`.
    File,
}

#[derive(Debug, Clone, Copy)]
pub struct Media {
    pub file_id: i32,
    pub kind: MediaKind,
}

/// A local file queued for sending.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub path: std::path::PathBuf,
    /// Image = send as photo, File = send as document.
    pub kind: MediaKind,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub sender_name: String,
    pub text: String,
    /// Unix timestamp (seconds)
    pub date: i64,
    pub time_str: String,
    pub is_outgoing: bool,
    pub is_read: bool,
    pub reply_to: Option<ReplyInfo>,
    pub media: Option<Media>,
}

/// `HH:MM` for messages.
pub fn format_message_time(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Compact chat-list timestamp: `HH:MM` today, `MM-DD` this year, else `YYYY-MM-DD`.
pub fn format_chat_time(ts: i64) -> String {
    let Some(dt) = DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local)) else {
        return String::new();
    };
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else if dt.year() == now.year() {
        dt.format("%m-%d").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}
