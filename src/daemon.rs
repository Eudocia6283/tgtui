//! Background service: owns the TDLib client, serves TUI clients over a Unix
//! socket and sends desktop notifications while no TUI is looking.

use crate::backend::model::Chat;
use crate::backend::mock::MockBackend;
use crate::backend::tdlib::{AuthState, TdlibBackend};
use crate::backend::{Auth, BackendEvent, TelegramBackend};
use crate::config::Config;
use crate::ipc::{self, Op, Reply, Request, ServerMsg};
use crate::notify::{NotifyAction, Notifier};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, oneshot};

/// Messages older than this when they arrive (backlog after reconnecting)
/// don't produce notifications.
const NOTIFY_MAX_AGE_SECS: i64 = 120;

/// Control messages from client tasks to the main loop.
enum Ctl {
    Connected { id: u64, tx: mpsc::UnboundedSender<ServerMsg>, pid: Option<u32> },
    Disconnected { id: u64 },
    Viewing { id: u64, chat_id: Option<i64> },
    Status { id: u64, reply: oneshot::Sender<Reply> },
    Shutdown,
}

/// What the service needs from a backend (TDLib in production, the mock
/// backend in tests).
pub trait ServiceBackend: TelegramBackend + Auth {
    /// No UI is attached any more: close the chat opened for it.
    fn close_opened_chat(&self) {}
}

impl ServiceBackend for TdlibBackend {
    fn close_opened_chat(&self) {
        TdlibBackend::close_opened_chat(self)
    }
}

impl ServiceBackend for MockBackend {}

struct Client {
    tx: mpsc::UnboundedSender<ServerMsg>,
    viewing: Option<i64>,
    pid: Option<u32>,
}

pub async fn run(config: Config) -> Result<()> {
    let tg = config
        .telegram
        .clone()
        .filter(|t| t.is_configured())
        .ok_or_else(|| anyhow!("no [telegram] api_id / api_hash configured"))?;

    let path = ipc::socket_path();
    if UnixStream::connect(&path).await.is_ok() {
        bail!("the background service is already running ({})", path.display());
    }
    let _ = std::fs::remove_file(&path);

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let backend = TdlibBackend::new(tg.api_id, tg.api_hash.clone(), tg.session_dir(), event_tx)?;

    let listener = UnixListener::bind(&path).with_context(|| format!("can't listen on {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("[daemon] pid {} listening on {}", std::process::id(), path.display());

    let (action_tx, action_rx) = mpsc::unbounded_channel();
    let notifier = Notifier::new(&config, action_tx).await;

    let result = serve(backend.clone(), listener, event_rx, Some((notifier, action_rx)), config).await;
    eprintln!("[daemon] shutting down");
    let _ = std::fs::remove_file(&path);
    backend.shutdown();
    result
}

/// The service loop: accept clients, forward events, notify, handle actions.
pub async fn serve(
    backend: Arc<dyn ServiceBackend>,
    listener: UnixListener,
    mut event_rx: mpsc::UnboundedReceiver<BackendEvent>,
    notifications: Option<(Notifier, mpsc::UnboundedReceiver<NotifyAction>)>,
    config: Config,
) -> Result<()> {
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel();
    let (mut notifier, mut action_rx) = match notifications {
        Some((n, rx)) => (Some(n), rx),
        None => (None, mpsc::unbounded_channel().1),
    };

    // Accept clients.
    {
        let backend = backend.clone();
        let ctl_tx = ctl_tx.clone();
        tokio::spawn(async move {
            let mut next_id = 1;
            while let Ok((stream, _)) = listener.accept().await {
                let id = next_id;
                next_id += 1;
                tokio::spawn(serve_client(id, stream, backend.clone(), ctl_tx.clone()));
            }
        });
    }

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    // Registering a handler keeps SIGHUP (terminal closed) from killing us.
    let mut sighup = signal(SignalKind::hangup())?;

    let mut auth = backend.auth_watch();
    let mut clients: HashMap<u64, Client> = HashMap::new();
    let mut chats: HashMap<i64, Chat> = HashMap::new();
    let mut online = false;

    loop {
        tokio::select! {
            Some(evt) = event_rx.recv() => {
                on_event(&evt, &mut chats, &clients, notifier.as_mut()).await;
                broadcast(&clients, || ServerMsg::Event(evt.clone()));
            }
            Some(ctl) = ctl_rx.recv() => match ctl {
                Ctl::Connected { id, tx, pid } => {
                    let _ = tx.send(ServerMsg::Auth(auth.borrow().clone()));
                    clients.insert(id, Client { tx, viewing: None, pid });
                }
                Ctl::Disconnected { id } => {
                    clients.remove(&id);
                    if clients.is_empty() {
                        backend.close_opened_chat();
                    }
                }
                Ctl::Viewing { id, chat_id } => {
                    if let Some(c) = clients.get_mut(&id) {
                        c.viewing = chat_id;
                    }
                    if let (Some(chat_id), Some(n)) = (chat_id, notifier.as_mut()) {
                        n.clear(chat_id).await;
                    }
                }
                Ctl::Status { id, reply } => {
                    let _ = reply.send(Reply::Status {
                        pid: std::process::id(),
                        // Not counting the connection asking.
                        clients: clients.keys().filter(|&&c| c != id).count(),
                        auth: auth.borrow().clone(),
                    });
                }
                Ctl::Shutdown => break,
            },
            Some(action) = action_rx.recv() => {
                if let Some(n) = notifier.as_mut() {
                    on_action(action, &*backend, &clients, &config, n).await;
                }
            }
            Ok(()) = auth.changed() => {
                let state = auth.borrow_and_update().clone();
                broadcast(&clients, || ServerMsg::Auth(state.clone()));
                if state == AuthState::Closed {
                    eprintln!("[daemon] TDLib closed");
                    break;
                }
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
            _ = sighup.recv() => {}
        }

        // Online only while some window is focused on a chat.
        let viewing = clients.values().find_map(|c| c.viewing);
        if viewing.is_some() != online {
            online = viewing.is_some();
            let _ = backend.set_viewing(viewing).await;
        }
    }

    Ok(())
}

fn broadcast(clients: &HashMap<u64, Client>, msg: impl Fn() -> ServerMsg) {
    for c in clients.values() {
        let _ = c.tx.send(msg());
    }
}

async fn on_event(
    evt: &BackendEvent,
    chats: &mut HashMap<i64, Chat>,
    clients: &HashMap<u64, Client>,
    notifier: Option<&mut Notifier>,
) {
    if let BackendEvent::ChatUpdated(chat) = evt {
        chats.insert(chat.id, chat.clone());
    }
    let Some(notifier) = notifier else { return };
    match evt {
        BackendEvent::ChatUpdated(chat) => {
            if chat.unread_count == 0 {
                notifier.clear(chat.id).await;
            }
        }
        BackendEvent::NewMessage(msg) => {
            let Some(chat) = chats.get(&msg.chat_id) else { return };
            let fresh = chrono::Utc::now().timestamp() - msg.date <= NOTIFY_MAX_AGE_SECS;
            let looking = clients.values().any(|c| c.viewing == Some(msg.chat_id));
            if !msg.is_outgoing && !chat.is_muted && fresh && !looking {
                notifier.message(chat, msg).await;
            }
        }
        _ => {}
    }
}

async fn on_action(
    action: NotifyAction,
    backend: &dyn ServiceBackend,
    clients: &HashMap<u64, Client>,
    config: &Config,
    notifier: &mut Notifier,
) {
    match action {
        NotifyAction::Open { chat_id, token } => {
            // Prefer an open TUI (the most recently connected one).
            if let Some((_, client)) = clients.iter().max_by_key(|(id, _)| **id) {
                let _ = client.tx.send(ServerMsg::Event(BackendEvent::OpenChat { chat_id }));
                if config.daemon.focus_window {
                    if let Some(pid) = client.pid {
                        focus_niri_window_of(pid).await;
                    }
                }
            } else {
                open_tui(config, chat_id, token);
            }
        }
        NotifyAction::MarkRead { chat_id, last_message_id } => {
            let _ = backend.mark_as_read(chat_id, last_message_id).await;
            notifier.clear(chat_id).await;
        }
        NotifyAction::Reply { chat_id, last_message_id, text } => {
            if let Err(e) = backend.send_message(chat_id, text, None).await {
                eprintln!("[daemon] reply failed: {e}");
            }
            let _ = backend.mark_as_read(chat_id, last_message_id).await;
            notifier.clear(chat_id).await;
        }
        NotifyAction::Closed { chat_id } => notifier.forget(chat_id),
    }
}

/// Launch a terminal with the TUI on `chat_id` (no TUI is connected).
fn open_tui(config: &Config, chat_id: i64, token: Option<String>) {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "telegram-tui".into());
    let line = config
        .daemon
        .open_command
        .replace("{exe}", &exe)
        .replace("{chat_id}", &chat_id.to_string());
    let mut parts = line.split_whitespace();
    let Some(program) = parts.next() else { return };
    let mut cmd = std::process::Command::new(program);
    cmd.args(parts)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Lets the compositor focus the new window (xdg-activation).
    if let Some(t) = token {
        cmd.env("XDG_ACTIVATION_TOKEN", t);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("[daemon] failed to run `{line}`: {e}"),
    }
}

/// On niri, focus the window whose process is an ancestor of `pid` (the
/// terminal running that TUI).
async fn focus_niri_window_of(pid: u32) {
    if std::env::var_os("NIRI_SOCKET").is_none() {
        return;
    }
    let Ok(out) = tokio::process::Command::new("niri").args(["msg", "--json", "windows"]).output().await else {
        return;
    };
    let Ok(windows) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return;
    };
    let ancestors = ancestors_of(pid);
    let window = windows.iter().find(|w| {
        w["pid"].as_u64().is_some_and(|p| ancestors.contains(&(p as u32)))
    });
    if let Some(id) = window.and_then(|w| w["id"].as_u64()) {
        let _ = tokio::process::Command::new("niri")
            .args(["msg", "action", "focus-window", "--id", &id.to_string()])
            .status()
            .await;
    }
}

fn ancestors_of(mut pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    while pid > 1 && out.len() < 64 {
        out.push(pid);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { break };
        // Field 4 (ppid) comes after the parenthesised command name.
        let ppid = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|p| p.parse().ok());
        match ppid {
            Some(p) => pid = p,
            None => break,
        }
    }
    out
}

async fn serve_client(id: u64, stream: UnixStream, backend: Arc<dyn ServiceBackend>, ctl: mpsc::UnboundedSender<Ctl>) {
    let pid = stream.peer_cred().ok().and_then(|c| c.pid()).map(|p| p as u32);
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ipc::write_msg(&mut write, &msg).await.is_err() {
                break;
            }
        }
    });

    let _ = ctl.send(Ctl::Connected { id, tx: tx.clone(), pid });
    let mut reader = ipc::reader(read);
    while let Ok(Some(req)) = ipc::read_msg::<Request>(&mut reader).await {
        let backend = backend.clone();
        let tx = tx.clone();
        let ctl = ctl.clone();
        // Run each request on its own task: downloads can take a while.
        tokio::spawn(async move {
            let result = handle(id, req.op, &backend, &ctl).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(ServerMsg::Response { id: req.id, result });
        });
    }
    let _ = ctl.send(Ctl::Disconnected { id });
    drop(tx);
    let _ = writer.await;
}

async fn handle(client: u64, op: Op, b: &Arc<dyn ServiceBackend>, ctl: &mpsc::UnboundedSender<Ctl>) -> Result<Reply> {
    Ok(match op {
        Op::Hello { version } => {
            if version != ipc::PROTOCOL_VERSION {
                bail!("protocol version {version} != {}", ipc::PROTOCOL_VERSION);
            }
            Reply::Unit
        }
        Op::GetChats => Reply::Chats(b.get_chats().await?),
        Op::GetMessages { chat_id, limit } => Reply::Messages(b.get_messages(chat_id, limit).await?),
        Op::LoadOlder { chat_id, from_message_id } => {
            b.load_older(chat_id, from_message_id).await?;
            Reply::Unit
        }
        Op::DownloadMedia { file_id } => Reply::Path(b.download_media(file_id).await?),
        Op::DownloadFile { file_id } => {
            b.download_file(file_id).await?;
            Reply::Unit
        }
        Op::SendMessage { chat_id, text, reply_to_id } => {
            b.send_message(chat_id, text, reply_to_id).await?;
            Reply::Unit
        }
        Op::SendFile { chat_id, file, caption, reply_to_id } => {
            b.send_file(chat_id, file, caption, reply_to_id).await?;
            Reply::Unit
        }
        Op::DeleteMessage { chat_id, message_id } => {
            b.delete_message(chat_id, message_id).await?;
            Reply::Unit
        }
        Op::MarkAsRead { chat_id, max_message_id } => {
            b.mark_as_read(chat_id, max_message_id).await?;
            Reply::Unit
        }
        Op::SetViewing { chat_id } => {
            let _ = ctl.send(Ctl::Viewing { id: client, chat_id });
            Reply::Unit
        }
        Op::SendPhone(p) => {
            b.send_phone_number(&p).await?;
            Reply::Unit
        }
        Op::SendCode(c) => {
            b.send_auth_code(&c).await?;
            Reply::Unit
        }
        Op::SendPassword(p) => {
            b.send_password(&p).await?;
            Reply::Unit
        }
        Op::TakeError => Reply::Error(b.take_error().await),
        Op::Status => {
            let (tx, rx) = oneshot::channel();
            let _ = ctl.send(Ctl::Status { id: client, reply: tx });
            rx.await?
        }
        Op::Shutdown => {
            let _ = ctl.send(Ctl::Shutdown);
            Reply::Unit
        }
    })
}

/// Write a systemd user unit that starts the service with the session.
pub fn install_service(config_path: Option<&std::path::Path>) -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let mut exec = format!("{} daemon", exe.display());
    if let Some(p) = config_path {
        exec.push_str(&format!(" --config {}", std::fs::canonicalize(p)?.display()));
    }
    let unit = format!(
        "[Unit]\n\
         Description=telegram-tui background service (Telegram connection + notifications)\n\
         After=graphical-session.target\n\
         PartOf=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart={exec}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    );
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("no config dir"))?
        .join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("telegram-tui.service");
    std::fs::write(&path, unit)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::remote::RemoteBackend;
    use std::time::Duration;

    async fn next_event(rx: &mut mpsc::UnboundedReceiver<BackendEvent>, pred: impl Fn(&BackendEvent) -> bool) -> BackendEvent {
        loop {
            let evt = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out").expect("closed");
            if pred(&evt) {
                return evt;
            }
        }
    }

    #[tokio::test]
    async fn notifies_only_fresh_unmuted_unwatched_incoming_messages() {
        use crate::backend::model::{ChatType, Message, SendState};
        let dir = std::env::temp_dir().join(format!("tgtui-notify-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls");
        let script = dir.join("fake-notify");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> {}\n", log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = Config::default();
        // Run through sh: exec'ing a just-written file can race with forks
        // in other test threads (ETXTBSY).
        config.wayland.notify_command = format!("sh {}", script.display());
        let mut notifier = Notifier::offline(&config);

        let chat = |id: i64, title: &str, muted: bool| Chat {
            id, title: title.into(), username: None, chat_type: ChatType::Group, unread_count: 1,
            is_pinned: false, is_muted: muted, last_message_preview: String::new(),
            last_message_time: String::new(), members_count: None, order: 1,
        };
        let now = chrono::Utc::now().timestamp();
        let msg = |chat_id: i64, id: i64, outgoing: bool, date: i64| Message {
            id, chat_id, sender_name: "Bob".into(), text: format!("m{id}"), date, time_str: String::new(),
            is_outgoing: outgoing, is_read: true, reply_to: None, media: None, send_state: SendState::Sent,
        };

        let mut chats = HashMap::new();
        let mut clients = HashMap::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        clients.insert(1, Client { tx, viewing: Some(3), pid: None });
        for c in [chat(2, "Arch", false), chat(3, "Watched", false), chat(5, "Muted", true)] {
            on_event(&BackendEvent::ChatUpdated(c), &mut chats, &clients, Some(&mut notifier)).await;
        }
        for (m, should) in [
            (msg(2, 1, false, now), true),         // fresh incoming
            (msg(2, 2, false, now), true),         // second one: same notification, "(2 new)"
            (msg(2, 3, true, now), false),         // our own
            (msg(5, 4, false, now), false),        // muted chat
            (msg(3, 5, false, now), false),        // a TUI is looking at it
            (msg(2, 6, false, now - 3600), false), // backlog
            (msg(9, 7, false, now), false),        // unknown chat
        ] {
            let before = std::fs::read_to_string(&log).unwrap_or_default().lines().count();
            on_event(&BackendEvent::NewMessage(m.clone()), &mut chats, &clients, Some(&mut notifier)).await;
            // The fallback command runs asynchronously.
            let mut after = before;
            for _ in 0..50 {
                after = std::fs::read_to_string(&log).unwrap_or_default().lines().count();
                if after > before || !should {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if !should {
                tokio::time::sleep(Duration::from_millis(50)).await;
                after = std::fs::read_to_string(&log).unwrap_or_default().lines().count();
            }
            assert_eq!(after > before, should, "message {} in chat {}", m.id, m.chat_id);
        }
        // The second message updates the same notification: count + both lines.
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.starts_with("Arch|Bob: m1\nArch (2 new)|Bob: m1\nBob: m2\n"), "{calls}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tui_client_talks_to_service_over_socket() {
        let dir = std::env::temp_dir().join(format!("tgtui-daemon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("svc.sock");
        let _ = std::fs::remove_file(&sock);

        let (etx, erx) = mpsc::unbounded_channel();
        let backend = Arc::new(MockBackend::new(etx));
        let listener = UnixListener::bind(&sock).unwrap();
        let service = tokio::spawn(serve(backend, listener, erx, None, Config::default()));

        // Two clients, like two TUI windows.
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let a = RemoteBackend::connect(&sock, tx1).await.unwrap();
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        let b = RemoteBackend::connect(&sock, tx2).await.unwrap();

        // Login state is pushed on connect.
        let mut auth = a.auth_watch();
        tokio::time::timeout(Duration::from_secs(5), auth.wait_for(|s| *s == AuthState::Ready)).await.unwrap().unwrap();

        assert_eq!(a.get_chats().await.unwrap().len(), 6);
        assert_eq!(a.get_messages(2, 50).await.unwrap().len(), 5);
        assert!(a.notifies(), "the service owns notifications");

        // A message sent by one client is broadcast to every client.
        a.send_message(2, "over the socket".into(), None).await.unwrap();
        for rx in [&mut rx1, &mut rx2] {
            let evt = next_event(rx, |e| matches!(e, BackendEvent::NewMessage(_))).await;
            let BackendEvent::NewMessage(m) = evt else { unreachable!() };
            assert_eq!(m.text, "over the socket");
        }

        // Errors from the backend come back as Err.
        assert!(b.download_media(1).await.unwrap_err().to_string().contains("demo mode"));

        b.set_viewing(Some(2)).await.unwrap();
        match crate::backend::remote::query_status(&sock).await.unwrap() {
            Reply::Status { clients, auth, .. } => {
                assert_eq!(auth, AuthState::Ready);
                assert_eq!(clients, 2, "the two TUI clients, not the status query");
            }
            _ => panic!("unexpected reply"),
        }

        // Shutdown stops the loop; clients notice.
        crate::backend::remote::request_shutdown(&sock).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), service).await.unwrap().unwrap().unwrap();
        drop((a, b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ancestors_include_self_and_parent() {
        let me = std::process::id();
        let chain = ancestors_of(me);
        assert_eq!(chain.first(), Some(&me));
        assert!(chain.contains(&std::os::unix::process::parent_id()));
    }
}
