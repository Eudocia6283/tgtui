//! Protocol between the background service (`telegram-tui daemon`) and TUI
//! clients: newline-delimited JSON over a Unix socket.

use crate::backend::model::{Attachment, Chat, Message};
use crate::backend::tdlib::AuthState;
use crate::backend::BackendEvent;
use anyhow::{anyhow, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Bumped when the protocol changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// `$XDG_RUNTIME_DIR/telegram-tui.sock` (private to the user).
pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("telegram-tui.sock")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: Op,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Op {
    Hello {
        version: u32,
    },
    GetChats,
    GetMessages {
        chat_id: i64,
        limit: usize,
    },
    LoadOlder {
        chat_id: i64,
        from_message_id: i64,
    },
    DownloadMedia {
        file_id: i32,
    },
    DownloadFile {
        file_id: i32,
    },
    SendMessage {
        chat_id: i64,
        text: String,
        reply_to_id: Option<i64>,
    },
    SendFile {
        chat_id: i64,
        file: Attachment,
        caption: String,
        reply_to_id: Option<i64>,
    },
    DeleteMessage {
        chat_id: i64,
        message_id: i64,
    },
    MarkAsRead {
        chat_id: i64,
        max_message_id: i64,
    },
    SetViewing {
        chat_id: Option<i64>,
    },
    SendPhone(String),
    SendCode(String),
    SendPassword(String),
    TakeError,
    Status,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Reply {
    Unit,
    Chats(Vec<Chat>),
    Messages(Vec<Message>),
    Path(PathBuf),
    Error(Option<String>),
    Status {
        pid: u32,
        clients: usize,
        auth: AuthState,
    },
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Response { id: u64, result: Result<Reply, String> },
    Event(BackendEvent),
    Auth(AuthState),
}

pub async fn write_msg<T: Serialize>(w: &mut OwnedWriteHalf, msg: &T) -> Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

pub type MsgReader = Lines<BufReader<OwnedReadHalf>>;

pub fn reader(r: OwnedReadHalf) -> MsgReader {
    BufReader::new(r).lines()
}

/// Next message, or None at EOF.
pub async fn read_msg<T: DeserializeOwned>(r: &mut MsgReader) -> Result<Option<T>> {
    match r.next_line().await? {
        Some(line) => serde_json::from_str(&line)
            .map(Some)
            .map_err(|e| anyhow!("bad message from peer: {e}")),
        None => Ok(None),
    }
}
