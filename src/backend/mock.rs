use super::model::*;
use super::tdlib::AuthState;
use super::{Auth, BackendEvent, TelegramBackend};
use anyhow::Result;
use chrono::Local;
use std::collections::HashMap;
use tokio::sync::{mpsc, Mutex};

pub struct MockBackend {
    chats: Mutex<Vec<Chat>>,
    messages: Mutex<HashMap<i64, Vec<Message>>>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
}

#[allow(clippy::too_many_arguments)]
fn chat(
    id: i64,
    title: &str,
    username: Option<&str>,
    chat_type: ChatType,
    unread_count: u32,
    is_pinned: bool,
    is_muted: bool,
    preview: &str,
    time: &str,
    members_count: Option<u32>,
) -> Chat {
    Chat {
        id,
        title: title.into(),
        username: username.map(Into::into),
        chat_type,
        unread_count,
        is_pinned,
        is_muted,
        last_message_preview: preview.into(),
        last_message_time: time.into(),
        members_count,
        // Keep the declared order: earlier entries sort higher.
        order: 100 - id,
    }
}

fn msg(id: i64, chat_id: i64, sender: &str, text: &str, time: &str, reply_to: Option<ReplyInfo>) -> Message {
    let is_outgoing = sender == "Me";
    Message {
        id,
        chat_id,
        sender_name: sender.into(),
        text: text.into(),
        date: 0,
        time_str: time.into(),
        is_outgoing,
        is_read: true,
        reply_to,
        media: None,
        send_state: SendState::Sent,
    }
}

impl MockBackend {
    pub fn new(event_tx: mpsc::UnboundedSender<BackendEvent>) -> Self {
        use ChatType::*;
        let chats = vec![
            chat(
                1,
                "Saved Messages",
                None,
                Direct,
                0,
                true,
                false,
                "TODO: update niri configuration",
                "12m",
                None,
            ),
            chat(
                2,
                "#archlinux-general",
                Some("archlinux_general"),
                Group,
                4,
                false,
                false,
                "Bob: Works like a charm on Arch!",
                "1m",
                Some(4821),
            ),
            chat(
                3,
                "Rustaceans Daily",
                Some("rust_daily"),
                Channel,
                0,
                false,
                false,
                "Ratatui 0.29 released with new widgets!",
                "35m",
                Some(18400),
            ),
            chat(
                4,
                "Alice Cooper",
                Some("alice_c"),
                Direct,
                1,
                false,
                false,
                "See you at the Linux meetup!",
                "2h",
                None,
            ),
            chat(
                5,
                "Noctalia Devs",
                Some("noctalia_project"),
                Group,
                0,
                false,
                true,
                "New shell widgets & quickshell merged",
                "5h",
                Some(342),
            ),
            chat(
                6,
                "Arch Linux News",
                Some("archlinux_news"),
                Channel,
                0,
                false,
                false,
                "Kernel 6.13 package rebuild announcements",
                "1d",
                Some(142000),
            ),
        ];

        let mut messages_map = HashMap::new();
        messages_map.insert(
            1,
            vec![
                msg(
                    1,
                    1,
                    "Me",
                    "Welcome to telegram-tui demo mode! Press ? for keybindings.",
                    "11:58",
                    None,
                ),
                msg(2, 1, "Me", "TODO: update niri configuration", "12:02", None),
                Message {
                    media: Some(Media {
                        file_id: 1,
                        kind: MediaKind::Image,
                        preview: None,
                    }),
                    ..msg(
                        3,
                        1,
                        "Me",
                        "[Photo] niri screenshot (press o to open)",
                        "12:05",
                        None,
                    )
                },
            ],
        );
        messages_map.insert(
            2,
            vec![
                msg(
                    101,
                    2,
                    "Alice",
                    "Has anyone tested running a modern TUI on niri with fish?",
                    "14:18",
                    None,
                ),
                msg(
                    102,
                    2,
                    "Dave",
                    "Yeah, niri's horizontal scrolling handles wide terminals smoothly.",
                    "14:20",
                    None,
                ),
                msg(
                    103,
                    2,
                    "Alice",
                    "Can you share your vim keybindings for quick navigation?",
                    "14:22",
                    None,
                ),
                msg(
                    104,
                    2,
                    "Bob",
                    "Works like magic! Especially if you inherit terminal ANSI 16 colors.",
                    "14:25",
                    Some(ReplyInfo {
                        sender_name: "Alice".into(),
                        text_preview: "Has anyone tested running a modern TUI on niri...".into(),
                    }),
                ),
                msg(
                    105,
                    2,
                    "Me",
                    "Yep, writing it in Rust with Ratatui right now!",
                    "14:26",
                    None,
                ),
            ],
        );
        messages_map.insert(
            4,
            vec![msg(
                401,
                4,
                "Alice Cooper",
                "See you at the Linux meetup!",
                "12:40",
                None,
            )],
        );

        let _ = event_tx.send(BackendEvent::ConnectionStatus("demo".into()));

        Self {
            chats: Mutex::new(chats),
            messages: Mutex::new(messages_map),
            event_tx,
        }
    }

    async fn push_outgoing(&self, chat_id: i64, text: String, reply_to_id: Option<i64>) {
        let now = Local::now();
        let mut msg_guard = self.messages.lock().await;
        let list = msg_guard.entry(chat_id).or_default();

        let new_id = list.last().map(|m| m.id + 1).unwrap_or(1);
        let reply_to = reply_to_id.and_then(|rid| {
            list.iter().find(|m| m.id == rid).map(|m| ReplyInfo {
                sender_name: m.sender_name.clone(),
                text_preview: m.text.chars().take(40).collect(),
            })
        });

        let new_msg = Message {
            id: new_id,
            chat_id,
            sender_name: "Me".into(),
            text,
            date: now.timestamp(),
            time_str: now.format("%H:%M").to_string(),
            is_outgoing: true,
            is_read: false,
            reply_to,
            media: None,
            send_state: SendState::Sent,
        };

        list.push(new_msg.clone());
        drop(msg_guard);

        let mut chats = self.chats.lock().await;
        if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
            c.last_message_preview = new_msg.text.clone();
            c.last_message_time = new_msg.time_str.clone();
            let _ = self.event_tx.send(BackendEvent::ChatUpdated(c.clone()));
        }
        let _ = self.event_tx.send(BackendEvent::NewMessage(new_msg));
    }
}

#[async_trait::async_trait]
impl TelegramBackend for MockBackend {
    async fn get_chats(&self) -> Result<Vec<Chat>> {
        Ok(self.chats.lock().await.clone())
    }

    async fn get_messages(&self, chat_id: i64, _limit: usize) -> Result<Vec<Message>> {
        Ok(self
            .messages
            .lock()
            .await
            .get(&chat_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn send_message(&self, chat_id: i64, text: String, reply_to_id: Option<i64>) -> Result<()> {
        self.push_outgoing(chat_id, text, reply_to_id).await;
        Ok(())
    }

    async fn send_file(
        &self,
        chat_id: i64,
        file: Attachment,
        caption: String,
        reply_to_id: Option<i64>,
    ) -> Result<()> {
        let label = match file.kind {
            MediaKind::Image => "Photo",
            MediaKind::File => "File",
        };
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = format!("[{label}: {name}] {caption}").trim_end().to_string();
        self.push_outgoing(chat_id, text, reply_to_id).await;
        Ok(())
    }

    async fn download_media(&self, _file_id: i32) -> Result<std::path::PathBuf> {
        Err(anyhow::anyhow!("media download is not available in demo mode"))
    }

    async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<()> {
        if let Some(list) = self.messages.lock().await.get_mut(&chat_id) {
            list.retain(|m| m.id != message_id);
        }
        let _ = self.event_tx.send(BackendEvent::MessagesDeleted {
            chat_id,
            message_ids: vec![message_id],
        });
        Ok(())
    }

    async fn mark_as_read(&self, chat_id: i64, _max_message_id: i64) -> Result<()> {
        if let Some(chat) = self.chats.lock().await.iter_mut().find(|c| c.id == chat_id) {
            chat.unread_count = 0;
        }
        Ok(())
    }
}

/// The demo account is always "logged in".
#[async_trait::async_trait]
impl Auth for MockBackend {
    fn auth_watch(&self) -> tokio::sync::watch::Receiver<AuthState> {
        tokio::sync::watch::channel(AuthState::Ready).1
    }
    async fn take_error(&self) -> Option<String> {
        None
    }
    async fn send_phone_number(&self, _phone: &str) -> Result<()> {
        Ok(())
    }
    async fn send_auth_code(&self, _code: &str) -> Result<()> {
        Ok(())
    }
    async fn send_password(&self, _password: &str) -> Result<()> {
        Ok(())
    }
}
