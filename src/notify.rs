//! Desktop notifications over D-Bus (org.freedesktop.Notifications, as
//! implemented by Noctalia): one notification per chat, with "Open" /
//! "Mark as read" actions and inline reply where the server supports it.
//! Falls back to `notify_command` (notify-send) without D-Bus.

use crate::backend::model::{Chat, ChatType, Message};
use crate::config::Config;
use futures::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use zbus::zvariant::Value;

/// Lines of recent messages shown in a chat's notification.
const MAX_LINES: usize = 4;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_replied(&self, id: u32, text: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn activation_token(&self, id: u32, activation_token: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

/// What the user did with one of our notifications.
#[derive(Debug)]
pub enum NotifyAction {
    /// Clicked the notification / "Open". `token` is an xdg-activation
    /// token for focusing the window we open.
    Open { chat_id: i64, token: Option<String> },
    MarkRead { chat_id: i64, last_message_id: i64 },
    Reply { chat_id: i64, last_message_id: i64, text: String },
    /// Dismissed (or expired) on the notification server's side.
    Closed { chat_id: i64 },
}

struct ChatNote {
    id: u32,
    lines: VecDeque<String>,
    count: u32,
    last_message_id: i64,
}

/// notification id -> (chat id, last message id)
type IdMap = Arc<Mutex<HashMap<u32, (i64, i64)>>>;

pub struct Notifier {
    proxy: Option<NotificationsProxy<'static>>,
    fallback_command: String,
    icon: String,
    inline_reply: bool,
    actions: bool,
    notes: HashMap<i64, ChatNote>,
    ids: IdMap,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Notifier {
    pub async fn new(config: &Config, action_tx: mpsc::UnboundedSender<NotifyAction>) -> Self {
        let ids: IdMap = Arc::default();
        let mut notifier = Self {
            proxy: None,
            fallback_command: config.wayland.notify_command.clone(),
            icon: config.daemon.notify_icon.clone(),
            inline_reply: false,
            actions: false,
            notes: HashMap::new(),
            ids: ids.clone(),
        };
        match Self::connect().await {
            Ok(proxy) => {
                let caps = proxy.get_capabilities().await.unwrap_or_default();
                notifier.actions = caps.iter().any(|c| c == "actions");
                notifier.inline_reply = config.daemon.inline_reply && caps.iter().any(|c| c == "inline-reply");
                eprintln!(
                    "[daemon] notifications via D-Bus (actions: {}, inline reply: {})",
                    notifier.actions, notifier.inline_reply
                );
                if let Err(e) = Self::listen(proxy.clone(), ids, action_tx).await {
                    eprintln!("[daemon] can't subscribe to notification signals: {e}");
                }
                notifier.proxy = Some(proxy);
            }
            Err(e) => eprintln!("[daemon] D-Bus notifications unavailable ({e}); using `{}`", notifier.fallback_command),
        }
        notifier
    }

    /// No D-Bus: always use `notify_command` (tests).
    #[cfg(test)]
    pub fn offline(config: &Config) -> Self {
        Self {
            proxy: None,
            fallback_command: config.wayland.notify_command.clone(),
            icon: config.daemon.notify_icon.clone(),
            inline_reply: false,
            actions: false,
            notes: HashMap::new(),
            ids: Arc::default(),
        }
    }

    async fn connect() -> zbus::Result<NotificationsProxy<'static>> {
        let conn = zbus::Connection::session().await?;
        NotificationsProxy::new(&conn).await
    }

    /// Forward signals for our notifications as `NotifyAction`s.
    async fn listen(
        proxy: NotificationsProxy<'static>,
        ids: IdMap,
        tx: mpsc::UnboundedSender<NotifyAction>,
    ) -> zbus::Result<()> {
        let mut actions = proxy.receive_action_invoked().await?;
        let mut replies = proxy.receive_notification_replied().await?;
        let mut tokens = proxy.receive_activation_token().await?;
        let mut closed = proxy.receive_notification_closed().await?;
        tokio::spawn(async move {
            // The activation token arrives right before ActionInvoked.
            let mut last_token: HashMap<u32, String> = HashMap::new();
            loop {
                tokio::select! {
                    Some(sig) = tokens.next() => {
                        if let Ok(a) = sig.args() {
                            last_token.insert(a.id, a.activation_token);
                        }
                    }
                    Some(sig) = actions.next() => {
                        let Ok(a) = sig.args() else { continue };
                        let Some((chat_id, last_message_id)) = lock(&ids).get(&a.id).copied() else { continue };
                        let action = match a.action_key.as_str() {
                            "mark-read" => NotifyAction::MarkRead { chat_id, last_message_id },
                            _ => NotifyAction::Open { chat_id, token: last_token.remove(&a.id) },
                        };
                        let _ = tx.send(action);
                    }
                    Some(sig) = replies.next() => {
                        let Ok(a) = sig.args() else { continue };
                        let Some((chat_id, last_message_id)) = lock(&ids).get(&a.id).copied() else { continue };
                        let _ = tx.send(NotifyAction::Reply { chat_id, last_message_id, text: a.text });
                    }
                    Some(sig) = closed.next() => {
                        let Ok(a) = sig.args() else { continue };
                        last_token.remove(&a.id);
                        if let Some((chat_id, _)) = lock(&ids).remove(&a.id) {
                            let _ = tx.send(NotifyAction::Closed { chat_id });
                        }
                    }
                    else => break,
                }
            }
        });
        Ok(())
    }

    /// Show (or update) the notification for `chat` with a new message.
    pub async fn message(&mut self, chat: &Chat, msg: &Message) {
        let text = msg.text.replace('\n', " ");
        let line = if chat.chat_type == ChatType::Group {
            format!("{}: {}", msg.sender_name, text)
        } else {
            text
        };

        let note = self.notes.entry(chat.id).or_insert(ChatNote {
            id: 0,
            lines: VecDeque::new(),
            count: 0,
            last_message_id: 0,
        });
        note.lines.push_back(line);
        if note.lines.len() > MAX_LINES {
            note.lines.pop_front();
        }
        note.count += 1;
        note.last_message_id = msg.id;

        let summary = if note.count > 1 {
            format!("{} ({} new)", chat.title, note.count)
        } else {
            chat.title.clone()
        };
        let body = note.lines.iter().cloned().collect::<Vec<_>>().join("\n");

        let Some(proxy) = &self.proxy else {
            let _ = crate::app::spawn_command(&self.fallback_command, &[summary, body], None);
            return;
        };

        let mut actions: Vec<&str> = Vec::new();
        if self.actions {
            actions.extend(["default", "Open", "open", "Open chat", "mark-read", "Mark as read"]);
        }
        let placeholder = format!("Reply to {}…", chat.title);
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("category", Value::from("im.received"));
        hints.insert("urgency", Value::U8(1));
        if self.inline_reply {
            actions.extend(["inline-reply", "Reply"]);
            hints.insert("x-kde-reply-placeholder-text", Value::from(placeholder.as_str()));
        }

        match proxy
            .notify("Telegram", note.id, &self.icon, &summary, &body, &actions, hints, -1)
            .await
        {
            Ok(id) => {
                let mut ids = lock(&self.ids);
                if note.id != id {
                    ids.remove(&note.id);
                }
                note.id = id;
                ids.insert(id, (chat.id, note.last_message_id));
            }
            Err(e) => eprintln!("[daemon] notify failed: {e}"),
        }
    }

    /// The chat was read (here or elsewhere): close its notification.
    pub async fn clear(&mut self, chat_id: i64) {
        if let Some(note) = self.notes.remove(&chat_id) {
            lock(&self.ids).remove(&note.id);
            if let Some(proxy) = &self.proxy {
                if note.id != 0 {
                    let _ = proxy.close_notification(note.id).await;
                }
            }
        }
    }

    /// The server already closed it; just drop our record.
    pub fn forget(&mut self, chat_id: i64) {
        self.notes.remove(&chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::SendState;

    /// Sends a real notification to the desktop: `cargo test live_ -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_dbus_notification() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut n = Notifier::new(&Config::default(), tx).await;
        let proxy = n.proxy.clone().expect("D-Bus notification server");
        assert!(n.actions, "server supports actions");
        let chat = Chat {
            id: -1, title: "telegram-tui test".into(), username: None, chat_type: ChatType::Direct,
            unread_count: 1, is_pinned: false, is_muted: false, last_message_preview: String::new(),
            last_message_time: String::new(), members_count: None, order: 1,
        };
        let msg = Message {
            id: 1, chat_id: -1, sender_name: "test".into(),
            text: "Background service notification check (closes in 8s)".into(), date: 0,
            time_str: String::new(), is_outgoing: false, is_read: false, reply_to: None, media: None,
            send_state: SendState::Sent,
        };
        n.message(&chat, &msg).await;
        let id = n.notes[&-1].id;
        assert!(id > 0, "Notify returned an id");
        println!("notification id {id}, inline reply: {}", n.inline_reply);
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        n.clear(-1).await;
        let _ = proxy;
    }
}
