//! TUI-side backend that talks to the background service over its socket.

use super::model::{Attachment, Chat, Message};
use super::tdlib::AuthState;
use super::{Auth, BackendEvent, TelegramBackend};
use crate::ipc::{self, Op, Reply, Request, ServerMsg};
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Reply, String>>>>>;

pub struct RemoteBackend {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU64,
    auth_rx: watch::Receiver<AuthState>,
}

impl RemoteBackend {
    /// Connect to a running service. Events are forwarded to `event_tx`.
    pub async fn connect(path: &Path, event_tx: mpsc::UnboundedSender<BackendEvent>) -> Result<Arc<Self>> {
        let stream = UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        let pending: Pending = Arc::default();
        let (auth_tx, auth_rx) = watch::channel(AuthState::WaitingParams);

        let pending_rx = pending.clone();
        tokio::spawn(async move {
            let mut reader = ipc::reader(read);
            loop {
                match ipc::read_msg::<ServerMsg>(&mut reader).await {
                    Ok(Some(ServerMsg::Response { id, result })) => {
                        if let Some(tx) = pending_rx.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    Ok(Some(ServerMsg::Event(evt))) => {
                        let _ = event_tx.send(evt);
                    }
                    Ok(Some(ServerMsg::Auth(state))) => {
                        auth_tx.send_replace(state);
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // Service went away: fail outstanding calls and tell the UI.
            pending_rx.lock().unwrap_or_else(|e| e.into_inner()).clear();
            auth_tx.send_replace(AuthState::Closed);
            let _ = event_tx.send(BackendEvent::ConnectionStatus("service stopped".into()));
            let _ = event_tx.send(BackendEvent::Error(
                "background service stopped; restart telegram-tui".into(),
            ));
        });

        let backend = Arc::new(Self {
            writer: tokio::sync::Mutex::new(write),
            pending,
            next_id: AtomicU64::new(1),
            auth_rx,
        });
        match backend.call(Op::Hello { version: ipc::PROTOCOL_VERSION }).await {
            Ok(_) => Ok(backend),
            Err(e) => bail!("service rejected connection: {e} (restart it with `telegram-tui daemon stop`)"),
        }
    }

    async fn call(&self, op: Op) -> Result<Reply> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        ipc::write_msg(&mut *self.writer.lock().await, &Request { id, op }).await?;
        rx.await
            .map_err(|_| anyhow!("background service disconnected"))?
            .map_err(|e| anyhow!(e))
    }

    async fn unit(&self, op: Op) -> Result<()> {
        self.call(op).await.map(|_| ())
    }
}

#[async_trait::async_trait]
impl TelegramBackend for RemoteBackend {
    async fn get_chats(&self) -> Result<Vec<Chat>> {
        match self.call(Op::GetChats).await? {
            Reply::Chats(c) => Ok(c),
            _ => bail!("unexpected reply"),
        }
    }

    async fn get_messages(&self, chat_id: i64, limit: usize) -> Result<Vec<Message>> {
        match self.call(Op::GetMessages { chat_id, limit }).await? {
            Reply::Messages(m) => Ok(m),
            _ => bail!("unexpected reply"),
        }
    }

    async fn load_older(&self, chat_id: i64, from_message_id: i64) -> Result<()> {
        self.unit(Op::LoadOlder { chat_id, from_message_id }).await
    }

    async fn download_media(&self, file_id: i32) -> Result<PathBuf> {
        match self.call(Op::DownloadMedia { file_id }).await? {
            Reply::Path(p) => Ok(p),
            _ => bail!("unexpected reply"),
        }
    }

    async fn download_file(&self, file_id: i32) -> Result<()> {
        self.unit(Op::DownloadFile { file_id }).await
    }

    async fn send_message(&self, chat_id: i64, text: String, reply_to_id: Option<i64>) -> Result<()> {
        self.unit(Op::SendMessage { chat_id, text, reply_to_id }).await
    }

    async fn send_file(&self, chat_id: i64, file: Attachment, caption: String, reply_to_id: Option<i64>) -> Result<()> {
        self.unit(Op::SendFile { chat_id, file, caption, reply_to_id }).await
    }

    async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<()> {
        self.unit(Op::DeleteMessage { chat_id, message_id }).await
    }

    async fn mark_as_read(&self, chat_id: i64, max_message_id: i64) -> Result<()> {
        self.unit(Op::MarkAsRead { chat_id, max_message_id }).await
    }

    async fn set_viewing(&self, chat_id: Option<i64>) -> Result<()> {
        self.unit(Op::SetViewing { chat_id }).await
    }

    fn notifies(&self) -> bool {
        true
    }
}

#[async_trait::async_trait]
impl Auth for RemoteBackend {
    fn auth_watch(&self) -> watch::Receiver<AuthState> {
        self.auth_rx.clone()
    }

    async fn take_error(&self) -> Option<String> {
        match self.call(Op::TakeError).await {
            Ok(Reply::Error(e)) => e,
            _ => None,
        }
    }

    async fn send_phone_number(&self, phone: &str) -> Result<()> {
        self.unit(Op::SendPhone(phone.to_string())).await
    }

    async fn send_auth_code(&self, code: &str) -> Result<()> {
        self.unit(Op::SendCode(code.to_string())).await
    }

    async fn send_password(&self, password: &str) -> Result<()> {
        self.unit(Op::SendPassword(password.to_string())).await
    }
}

/// Ask a running service for its status (pid, clients, login state).
pub async fn query_status(path: &Path) -> Result<Reply> {
    let (tx, _rx) = mpsc::unbounded_channel();
    let backend = RemoteBackend::connect(path, tx).await?;
    backend.call(Op::Status).await
}

/// Ask a running service to shut down.
pub async fn request_shutdown(path: &Path) -> Result<()> {
    let (tx, _rx) = mpsc::unbounded_channel();
    let backend = RemoteBackend::connect(path, tx).await?;
    // The service may close the socket before answering.
    let _ = backend.call(Op::Shutdown).await;
    Ok(())
}
