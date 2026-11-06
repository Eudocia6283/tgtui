pub mod mock;
pub mod model;
pub mod tdlib;

use anyhow::{anyhow, Result};
use model::{Attachment, Chat, Message};
use std::path::PathBuf;

pub enum BackendEvent {
    /// A brand-new message (incoming or our own pending one).
    NewMessage(Message),
    /// A message was replaced (send succeeded and got a server id, or edited).
    MessageUpdated { old_id: i64, message: Message },
    /// Messages were permanently deleted.
    MessagesDeleted { chat_id: i64, message_ids: Vec<i64> },
    /// Our outgoing messages up to `message_id` have been read by the peer.
    OutboxRead { chat_id: i64, message_id: i64 },
    /// Full (sorted) history snapshot for a chat.
    History { chat_id: i64, messages: Vec<Message> },
    /// Chat created or changed (order 0 = removed from the main list).
    ChatUpdated(Chat),
    ConnectionStatus(String),
    Error(String),
    /// Informational message for the status bar.
    Status(String),
    /// A file requested with `download_file` finished downloading.
    FileDownloaded { file_id: i32, path: PathBuf },
    /// An inline image finished decoding (sent by the app's own worker).
    ImageDecoded { file_id: i32, result: std::result::Result<crate::images::DecodedImage, String> },
}

#[async_trait::async_trait]
pub trait TelegramBackend: Send + Sync {
    async fn get_chats(&self) -> Result<Vec<Chat>>;
    /// Returns cached messages immediately; backends may also deliver a
    /// `BackendEvent::History` later with fresher data.
    async fn get_messages(&self, chat_id: i64, limit: usize) -> Result<Vec<Message>>;
    /// Request messages older than `from_message_id` (delivered via `History`).
    async fn load_older(&self, _chat_id: i64, _from_message_id: i64) -> Result<()> {
        Ok(())
    }
    /// Download a media file (blocking until done) and return its local path.
    async fn download_media(&self, _file_id: i32) -> Result<PathBuf> {
        Err(anyhow!("media download is not supported by this backend"))
    }
    /// Start downloading a file in the background; completion is reported
    /// with `BackendEvent::FileDownloaded`.
    async fn download_file(&self, _file_id: i32) -> Result<()> {
        Ok(())
    }
    async fn send_message(&self, chat_id: i64, text: String, reply_to_id: Option<i64>) -> Result<()>;
    /// Send a local file as photo/document with an optional caption.
    async fn send_file(
        &self,
        chat_id: i64,
        file: Attachment,
        caption: String,
        reply_to_id: Option<i64>,
    ) -> Result<()>;
    async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<()>;
    async fn mark_as_read(&self, chat_id: i64, max_message_id: i64) -> Result<()>;
}
