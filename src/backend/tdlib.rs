use super::model::{
    format_chat_time, format_message_time, Attachment, Chat, ChatType, Media, MediaKind, Message,
    Preview, ReplyInfo, SendState,
};
use super::{BackendEvent, TelegramBackend};
use anyhow::{anyhow, bail, Result};
use libloading::{Library, Symbol};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

type TdCreate = unsafe extern "C" fn() -> *mut c_void;
type TdSend = unsafe extern "C" fn(*mut c_void, *const c_char);
type TdReceive = unsafe extern "C" fn(*mut c_void, c_double) -> *const c_char;
type TdExecute = unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char;
type TdDestroy = unsafe extern "C" fn(*mut c_void);

/// Number of messages we try to have cached when a chat is opened.
const HISTORY_TARGET: usize = 50;
/// Upper bound for a single media download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    WaitingParams,
    WaitingPhone,
    WaitingCode,
    WaitingPassword,
    Ready,
    Closed,
    Other(String),
}

/// Raw TDLib client handle. The td_json_client API allows `send` from any
/// thread concurrently with `receive` on another thread.
#[derive(Clone, Copy)]
struct Client {
    ptr: *mut c_void,
    send_fn: TdSend,
}

unsafe impl Send for Client {}
unsafe impl Sync for Client {}

impl Client {
    fn send(&self, req: &Value) {
        if let Ok(s) = CString::new(req.to_string()) {
            unsafe { (self.send_fn)(self.ptr, s.as_ptr()) }
        }
    }
}

struct User {
    name: String,
    username: Option<String>,
    is_bot: bool,
}

#[derive(Default)]
struct State {
    chats: HashMap<i64, Chat>,
    users: HashMap<i64, User>,
    /// user id -> private chat id
    user_chats: HashMap<i64, i64>,
    /// Sorted ascending by message id.
    messages: HashMap<i64, Vec<Message>>,
    outbox_read: HashMap<i64, i64>,
    /// Errors that arrive before login completes (shown by the login prompt).
    last_error: Option<String>,
    /// Requests awaiting a response, keyed by the id in `@extra = "req:<id>"`.
    pending: HashMap<u64, oneshot::Sender<Value>>,
    /// Files requested via `download_file`.
    wanted_downloads: std::collections::HashSet<i32>,
    /// Last reported upload progress (in 10% steps) per file.
    upload_progress: HashMap<i32, u64>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct TdlibBackend {
    client: Client,
    state: Arc<Mutex<State>>,
    auth_rx: watch::Receiver<AuthState>,
    closed: Arc<AtomicBool>,
    opened_chat: Mutex<Option<i64>>,
    next_request_id: AtomicU64,
    _lib: Arc<Library>,
}

impl TdlibBackend {
    pub fn new(
        api_id: i32,
        api_hash: String,
        session_dir: PathBuf,
        event_tx: mpsc::UnboundedSender<BackendEvent>,
    ) -> Result<Arc<Self>> {
        // Load the locally installed TDLib first, then fall back to the
        // system paths / dynamic loader. `~/.local/share/telegram-tui/tdlib-lib`
        // is where the no-root installer places libtdjson.so on Arch.
        let local_tdlib = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("telegram-tui")
            .join("tdlib-lib")
            .join("libtdjson.so");
        let lib = Arc::new(unsafe {
            Library::new(&local_tdlib)
                .or_else(|_| Library::new("/usr/lib/libtdjson.so"))
                .or_else(|_| Library::new("libtdjson.so"))
                .map_err(|e| {
                    anyhow!(
                        "Could not load libtdjson.so. Tried:\n  {}\n  /usr/lib/libtdjson.so\n  libtdjson.so\nOn Arch Linux install the AUR package `telegram-tdlib`, or place libtdjson.so in ~/.local/share/telegram-tui/tdlib-lib.\nError: {}",
                        local_tdlib.display(),
                        e
                    )
                })?
        });

        std::fs::create_dir_all(&session_dir)?;

        // Keep TDLib's logging off the terminal (it would corrupt the TUI):
        // send it to a file inside the session directory instead.
        if let Ok(execute) = unsafe { lib.get::<TdExecute>(b"td_json_client_execute") } {
            let log_path = session_dir.join("tdlib.log");
            for req in [
                json!({ "@type": "setLogVerbosityLevel", "new_verbosity_level": 1 }),
                json!({
                    "@type": "setLogStream",
                    "log_stream": {
                        "@type": "logStreamFile",
                        "path": log_path.to_string_lossy(),
                        "max_file_size": 10 * 1024 * 1024,
                        "redirect_stderr": false
                    }
                }),
            ] {
                if let Ok(s) = CString::new(req.to_string()) {
                    unsafe { execute(std::ptr::null_mut(), s.as_ptr()) };
                }
            }
        } else if let Ok(set_level) =
            unsafe { lib.get::<unsafe extern "C" fn(i32)>(b"td_set_log_verbosity_level") }
        {
            unsafe { set_level(1) };
        }

        let create_fn: Symbol<TdCreate> = unsafe { lib.get(b"td_json_client_create")? };
        let send_fn: Symbol<TdSend> = unsafe { lib.get(b"td_json_client_send")? };
        let receive_fn: Symbol<TdReceive> = unsafe { lib.get(b"td_json_client_receive")? };
        let destroy_fn: Symbol<TdDestroy> = unsafe { lib.get(b"td_json_client_destroy")? };

        let client = Client {
            ptr: unsafe { create_fn() },
            send_fn: *send_fn,
        };

        let state = Arc::new(Mutex::new(State::default()));
        let (auth_tx, auth_rx) = watch::channel(AuthState::WaitingParams);
        let closed = Arc::new(AtomicBool::new(false));

        let worker = Worker {
            client,
            receive_fn: *receive_fn,
            destroy_fn: *destroy_fn,
            state: state.clone(),
            auth_tx,
            event_tx,
            closed: closed.clone(),
            api_id,
            api_hash,
            session_dir,
            _lib: lib.clone(),
        };
        std::thread::Builder::new()
            .name("tdlib-receive".into())
            .spawn(move || worker.run())?;

        Ok(Arc::new(Self {
            client,
            state,
            auth_rx,
            closed,
            opened_chat: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            _lib: lib,
        }))
    }

    pub fn auth_watch(&self) -> watch::Receiver<AuthState> {
        self.auth_rx.clone()
    }

    pub fn take_error(&self) -> Option<String> {
        lock(&self.state).last_error.take()
    }

    pub fn send_phone_number(&self, phone: &str) {
        self.client.send(&json!({
            "@type": "setAuthenticationPhoneNumber",
            "phone_number": phone.trim()
        }));
    }

    pub fn send_auth_code(&self, code: &str) {
        self.client.send(&json!({
            "@type": "checkAuthenticationCode",
            "code": code.trim()
        }));
    }

    pub fn send_password(&self, password: &str) {
        self.client.send(&json!({
            "@type": "checkAuthenticationPassword",
            "password": password.trim_end_matches(['\r', '\n'])
        }));
    }

    /// Send a request and wait for its response (TDLib errors become `Err`).
    async fn request(&self, mut req: Value) -> Result<Value> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.state).pending.insert(id, tx);
        req["@extra"] = json!(format!("req:{id}"));
        self.client.send(&req);
        let resp = rx.await.map_err(|_| anyhow!("TDLib client closed"))?;
        if resp["@type"] == "error" {
            bail!("{} ({})", resp["message"].as_str().unwrap_or("unknown error"), resp["code"]);
        }
        Ok(resp)
    }

    /// TDLib answers with updateNewMessage (pending) and later
    /// updateMessageSendSucceeded, which the receive thread forwards.
    fn send_content(&self, chat_id: i64, content: Value, reply_to_id: Option<i64>) {
        let mut req = json!({
            "@type": "sendMessage",
            "chat_id": chat_id,
            "input_message_content": content
        });
        if let Some(rid) = reply_to_id {
            req["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": rid
            });
        }
        self.client.send(&req);
    }

    fn request_history(&self, chat_id: i64, from_message_id: i64, limit: usize, tag: &str) {
        self.client.send(&json!({
            "@type": "getChatHistory",
            "chat_id": chat_id,
            "from_message_id": from_message_id,
            "offset": 0,
            "limit": limit,
            "only_local": false,
            "@extra": format!("{tag}:{chat_id}")
        }));
    }
}

impl Drop for TdlibBackend {
    fn drop(&mut self) {
        // The receive thread owns the client and destroys it once TDLib
        // reports `authorizationStateClosed`; destroying it here while the
        // thread is inside `receive` would be a use-after-free.
        if !self.closed.load(Ordering::SeqCst) {
            self.client.send(&json!({ "@type": "close" }));
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.closed.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[async_trait::async_trait]
impl TelegramBackend for TdlibBackend {
    async fn get_chats(&self) -> Result<Vec<Chat>> {
        let st = lock(&self.state);
        let mut chats: Vec<Chat> = st.chats.values().filter(|c| c.order != 0).cloned().collect();
        chats.sort_by_key(|c| std::cmp::Reverse(c.order));
        Ok(chats)
    }

    async fn get_messages(&self, chat_id: i64, limit: usize) -> Result<Vec<Message>> {
        {
            let mut opened = self.opened_chat.lock().unwrap_or_else(|e| e.into_inner());
            if *opened != Some(chat_id) {
                if let Some(prev) = opened.replace(chat_id) {
                    self.client.send(&json!({ "@type": "closeChat", "chat_id": prev }));
                }
                self.client.send(&json!({ "@type": "openChat", "chat_id": chat_id }));
            }
        }
        self.request_history(chat_id, 0, limit, "history");
        Ok(lock(&self.state).messages.get(&chat_id).cloned().unwrap_or_default())
    }

    async fn load_older(&self, chat_id: i64, from_message_id: i64) -> Result<()> {
        self.request_history(chat_id, from_message_id, HISTORY_TARGET, "older");
        Ok(())
    }

    async fn download_file(&self, file_id: i32) -> Result<()> {
        lock(&self.state).wanted_downloads.insert(file_id);
        self.client.send(&json!({
            "@type": "downloadFile",
            "file_id": file_id,
            "priority": 1,
            "offset": 0,
            "limit": 0,
            "synchronous": false
        }));
        Ok(())
    }

    async fn download_media(&self, file_id: i32) -> Result<PathBuf> {
        let req = self.request(json!({
            "@type": "downloadFile",
            "file_id": file_id,
            "priority": 32,
            "offset": 0,
            "limit": 0,
            "synchronous": true
        }));
        let file = match tokio::time::timeout(DOWNLOAD_TIMEOUT, req).await {
            Ok(res) => res?,
            Err(_) => {
                self.client.send(&json!({ "@type": "cancelDownloadFile", "file_id": file_id }));
                bail!("download timed out");
            }
        };
        let path = file["local"]["path"].as_str().unwrap_or("");
        if !file["local"]["is_downloading_completed"].as_bool().unwrap_or(false) || path.is_empty() {
            bail!("download did not complete");
        }
        Ok(PathBuf::from(path))
    }

    async fn send_message(&self, chat_id: i64, text: String, reply_to_id: Option<i64>) -> Result<()> {
        self.send_content(
            chat_id,
            json!({ "@type": "inputMessageText", "text": formatted_text(&text) }),
            reply_to_id,
        );
        Ok(())
    }

    async fn send_file(
        &self,
        chat_id: i64,
        file: Attachment,
        caption: String,
        reply_to_id: Option<i64>,
    ) -> Result<()> {
        if !file.path.is_file() {
            bail!("file not found: {}", file.path.display());
        }
        let input_file = json!({ "@type": "inputFileLocal", "path": file.path.to_string_lossy() });
        // Photos must stay within Telegram's limits; otherwise send as a file.
        let dims = image::image_dimensions(&file.path).ok();
        let size = std::fs::metadata(&file.path).map(|m| m.len()).unwrap_or(0);
        let photo_ok = dims.is_some_and(|(w, h)| w + h <= 10_000 && w.max(h) <= 20 * w.min(h).max(1))
            && size <= 10 * 1024 * 1024;
        let kind = if file.kind == MediaKind::Image && !photo_ok { MediaKind::File } else { file.kind };
        let content = match kind {
            MediaKind::Image => {
                let (w, h) = dims.unwrap_or((0, 0));
                json!({
                    "@type": "inputMessagePhoto",
                    "photo": input_file,
                    "width": w,
                    "height": h,
                    "caption": formatted_text(&caption)
                })
            }
            MediaKind::File => json!({
                "@type": "inputMessageDocument",
                "document": input_file,
                "caption": formatted_text(&caption)
            }),
        };
        self.send_content(chat_id, content, reply_to_id);
        Ok(())
    }

    async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<()> {
        self.client.send(&json!({
            "@type": "deleteMessages",
            "chat_id": chat_id,
            "message_ids": [message_id],
            "revoke": true
        }));
        Ok(())
    }

    async fn mark_as_read(&self, chat_id: i64, max_message_id: i64) -> Result<()> {
        if max_message_id != 0 {
            self.client.send(&json!({
                "@type": "viewMessages",
                "chat_id": chat_id,
                "message_ids": [max_message_id],
                "source": { "@type": "messageSourceChatHistory" },
                "force_read": true
            }));
        }
        Ok(())
    }
}

/// Background receive loop: owns the TDLib client until it is closed.
struct Worker {
    client: Client,
    receive_fn: TdReceive,
    destroy_fn: TdDestroy,
    state: Arc<Mutex<State>>,
    auth_tx: watch::Sender<AuthState>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
    closed: Arc<AtomicBool>,
    api_id: i32,
    api_hash: String,
    session_dir: PathBuf,
    _lib: Arc<Library>,
}

unsafe impl Send for Worker {}

impl Worker {
    fn run(self) {
        loop {
            let ptr = unsafe { (self.receive_fn)(self.client.ptr, 1.0) };
            if ptr.is_null() {
                continue;
            }
            // The returned string is only valid until the next receive call.
            let Ok(raw) = unsafe { CStr::from_ptr(ptr) }.to_str() else {
                continue;
            };
            let Ok(val) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            if !self.handle(&val) {
                break;
            }
        }
        self.closed.store(true, Ordering::SeqCst);
        unsafe { (self.destroy_fn)(self.client.ptr) };
    }

    fn emit(&self, evt: BackendEvent) {
        let _ = self.event_tx.send(evt);
    }

    /// Apply `f` to a known chat and broadcast the new snapshot.
    fn update_chat(&self, chat_id: i64, f: impl FnOnce(&mut Chat)) {
        let snapshot = {
            let mut st = lock(&self.state);
            st.chats.get_mut(&chat_id).map(|c| {
                f(c);
                c.clone()
            })
        };
        if let Some(c) = snapshot {
            self.emit(BackendEvent::ChatUpdated(c));
        }
    }

    /// Returns false once the client is closed.
    fn handle(&self, val: &Value) -> bool {
        // Responses to `TdlibBackend::request`.
        if let Some(id) = val["@extra"]
            .as_str()
            .and_then(|e| e.strip_prefix("req:"))
            .and_then(|n| n.parse::<u64>().ok())
        {
            if let Some(tx) = lock(&self.state).pending.remove(&id) {
                let _ = tx.send(val.clone());
            }
            return true;
        }

        match val["@type"].as_str().unwrap_or("") {
            "updateAuthorizationState" => return self.on_auth(&val["authorization_state"]),

            "updateUser" => {
                let u = &val["user"];
                let id = u["id"].as_i64().unwrap_or(0);
                let name = format!(
                    "{} {}",
                    u["first_name"].as_str().unwrap_or(""),
                    u["last_name"].as_str().unwrap_or("")
                )
                .trim()
                .to_string();
                let user = User {
                    name: if name.is_empty() { format!("User {id}") } else { name },
                    username: u["usernames"]["active_usernames"][0]
                        .as_str()
                        .or_else(|| u["username"].as_str())
                        .filter(|s| !s.is_empty())
                        .map(Into::into),
                    is_bot: u["type"]["@type"] == "userTypeBot",
                };
                let chat_id = {
                    let mut st = lock(&self.state);
                    let username = user.username.clone();
                    let is_bot = user.is_bot;
                    st.users.insert(id, user);
                    let chat_id = st.user_chats.get(&id).copied();
                    if let Some(c) = chat_id.and_then(|cid| st.chats.get_mut(&cid)) {
                        c.username = username;
                        if is_bot {
                            c.chat_type = ChatType::Bot;
                        }
                    }
                    chat_id
                };
                if let Some(cid) = chat_id {
                    self.update_chat(cid, |_| {});
                }
            }

            "updateNewChat" => {
                let c = &val["chat"];
                let chat = {
                    let mut st = lock(&self.state);
                    let chat = build_chat(&mut st, c);
                    st.chats.insert(chat.id, chat.clone());
                    chat
                };
                self.emit(BackendEvent::ChatUpdated(chat));
            }

            "updateChatTitle" => {
                let title = val["title"].as_str().unwrap_or("").to_string();
                self.update_chat(chat_id_of(val), |c| c.title = title);
            }

            "updateChatPosition" => {
                let pos = val["position"].clone();
                self.update_chat(chat_id_of(val), |c| apply_position(c, &pos));
            }

            "updateChatLastMessage" => {
                let chat_id = chat_id_of(val);
                let (preview, time) = {
                    let st = lock(&self.state);
                    last_message_summary(&st, chat_id, &val["last_message"])
                };
                let positions = val["positions"].clone();
                self.update_chat(chat_id, |c| {
                    c.last_message_preview = preview;
                    c.last_message_time = time;
                    apply_positions(c, &positions);
                });
            }

            "updateChatReadInbox" => {
                let unread = val["unread_count"].as_u64().unwrap_or(0) as u32;
                self.update_chat(chat_id_of(val), |c| c.unread_count = unread);
            }

            "updateChatReadOutbox" => {
                let chat_id = chat_id_of(val);
                let last_read = val["last_read_outbox_message_id"].as_i64().unwrap_or(0);
                {
                    let mut st = lock(&self.state);
                    st.outbox_read.insert(chat_id, last_read);
                    if let Some(list) = st.messages.get_mut(&chat_id) {
                        for m in list.iter_mut().filter(|m| m.is_outgoing && m.id <= last_read) {
                            m.is_read = true;
                        }
                    }
                }
                self.emit(BackendEvent::OutboxRead { chat_id, message_id: last_read });
            }

            "updateChatNotificationSettings" => {
                let muted = val["notification_settings"]["mute_for"].as_i64().unwrap_or(0) > 0;
                self.update_chat(chat_id_of(val), |c| c.is_muted = muted);
            }

            "updateNewMessage" => {
                let msg = {
                    let mut st = lock(&self.state);
                    let msg = parse_message(&st, &val["message"]);
                    let list = st.messages.entry(msg.chat_id).or_default();
                    if !upsert_message(list, msg.clone()) {
                        return true;
                    }
                    msg
                };
                self.emit(BackendEvent::NewMessage(msg));
            }

            "updateMessageSendSucceeded" => {
                let old_id = val["old_message_id"].as_i64().unwrap_or(0);
                let msg = {
                    let mut st = lock(&self.state);
                    let msg = parse_message(&st, &val["message"]);
                    let list = st.messages.entry(msg.chat_id).or_default();
                    list.retain(|m| m.id != old_id);
                    upsert_message(list, msg.clone());
                    msg
                };
                self.emit(BackendEvent::MessageUpdated { old_id, message: msg });
            }

            "updateMessageContent" => {
                let chat_id = chat_id_of(val);
                let message_id = val["message_id"].as_i64().unwrap_or(0);
                let updated = {
                    let mut st = lock(&self.state);
                    st.messages.get_mut(&chat_id).and_then(|list| {
                        list.iter_mut().find(|m| m.id == message_id).map(|m| {
                            m.text = content_text(&val["new_content"]);
                            m.media = content_media(&val["new_content"]);
                            m.clone()
                        })
                    })
                };
                if let Some(message) = updated {
                    self.emit(BackendEvent::MessageUpdated { old_id: message_id, message });
                }
            }

            "updateMessageSendFailed" => {
                let old_id = val["old_message_id"].as_i64().unwrap_or(0);
                let err = error_text(&val["error"], val);
                let msg = {
                    let mut st = lock(&self.state);
                    let mut msg = parse_message(&st, &val["message"]);
                    msg.send_state = SendState::Failed(err.clone());
                    let list = st.messages.entry(msg.chat_id).or_default();
                    list.retain(|m| m.id != old_id);
                    upsert_message(list, msg.clone());
                    msg
                };
                self.emit(BackendEvent::MessageUpdated { old_id, message: msg });
                self.emit(BackendEvent::Error(format!("Send failed: {err}")));
            }

            "updateFile" => self.on_file(&val["file"]),

            "updateDeleteMessages" => {
                if val["is_permanent"].as_bool().unwrap_or(false) {
                    let chat_id = chat_id_of(val);
                    let message_ids: Vec<i64> = val["message_ids"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_i64).collect())
                        .unwrap_or_default();
                    if let Some(list) = lock(&self.state).messages.get_mut(&chat_id) {
                        list.retain(|m| !message_ids.contains(&m.id));
                    }
                    self.emit(BackendEvent::MessagesDeleted { chat_id, message_ids });
                }
            }

            "updateConnectionState" => {
                let label = match val["state"]["@type"].as_str().unwrap_or("") {
                    "connectionStateReady" => "online",
                    "connectionStateUpdating" => "updating…",
                    "connectionStateConnecting" | "connectionStateConnectingToProxy" => "connecting…",
                    "connectionStateWaitingForNetwork" => "waiting for network",
                    _ => "unknown",
                };
                self.emit(BackendEvent::ConnectionStatus(label.into()));
            }

            "messages" => self.on_history(val),

            "error" => self.on_error(val),

            _ => {}
        }
        true
    }

    fn on_file(&self, file: &Value) {
        let Some(id) = file["id"].as_i64().map(|i| i as i32) else {
            return;
        };
        let local = &file["local"];
        let remote = &file["remote"];

        if local["is_downloading_completed"].as_bool() == Some(true) {
            let wanted = lock(&self.state).wanted_downloads.remove(&id);
            if let (true, Some(path)) = (wanted, local["path"].as_str()) {
                self.emit(BackendEvent::FileDownloaded { file_id: id, path: PathBuf::from(path) });
            }
        }

        // Upload progress for our own media messages.
        if remote["is_uploading_active"].as_bool() == Some(true) {
            let total = file["size"].as_u64().or_else(|| file["expected_size"].as_u64()).unwrap_or(0);
            let done = remote["uploaded_size"].as_u64().unwrap_or(0);
            if let Some(step) = (done * 10).checked_div(total) {
                let prev = lock(&self.state).upload_progress.insert(id, step);
                if prev != Some(step) {
                    self.emit(BackendEvent::Status(format!("Uploading… {}%", step * 10)));
                }
            }
        } else if remote["is_uploading_completed"].as_bool() == Some(true)
            && lock(&self.state).upload_progress.remove(&id).is_some()
        {
            self.emit(BackendEvent::Status("Upload complete".into()));
        }
    }

    fn on_auth(&self, auth: &Value) -> bool {
        let new_state = match auth["@type"].as_str().unwrap_or("") {
            "authorizationStateWaitTdlibParameters" => {
                self.client.send(&json!({
                    "@type": "setTdlibParameters",
                    "database_directory": self.session_dir.to_string_lossy(),
                    "files_directory": self.session_dir.join("files").to_string_lossy(),
                    "use_file_database": true,
                    "use_chat_info_database": true,
                    "use_message_database": true,
                    "use_secret_chats": false,
                    "api_id": self.api_id,
                    "api_hash": self.api_hash,
                    "system_language_code": "en",
                    "device_model": "Arch Linux (niri)",
                    "system_version": "Linux",
                    "application_version": env!("CARGO_PKG_VERSION"),
                }));
                AuthState::WaitingParams
            }
            "authorizationStateWaitPhoneNumber" => AuthState::WaitingPhone,
            "authorizationStateWaitCode" => AuthState::WaitingCode,
            "authorizationStateWaitPassword" => AuthState::WaitingPassword,
            "authorizationStateReady" => {
                self.client.send(&json!({
                    "@type": "loadChats",
                    "chat_list": { "@type": "chatListMain" },
                    "limit": 100,
                    "@extra": "loadChats"
                }));
                AuthState::Ready
            }
            "authorizationStateClosed" => {
                self.auth_tx.send_replace(AuthState::Closed);
                return false;
            }
            other => AuthState::Other(other.to_string()),
        };
        self.auth_tx.send_replace(new_state);
        true
    }

    fn on_history(&self, val: &Value) {
        let extra = val["@extra"].as_str().unwrap_or("");
        let Some((tag, id)) = extra.split_once(':') else {
            return;
        };
        let Ok(chat_id) = id.parse::<i64>() else {
            return;
        };
        let batch = val["messages"].as_array().cloned().unwrap_or_default();

        let (snapshot, oldest) = {
            let mut st = lock(&self.state);
            let parsed: Vec<Message> = batch.iter().map(|m| parse_message(&st, m)).collect();
            let list = st.messages.entry(chat_id).or_default();
            for m in parsed {
                upsert_message(list, m);
            }
            (list.clone(), list.first().map(|m| m.id))
        };

        // TDLib often returns only what is cached locally on the first call;
        // keep fetching until we have a reasonable amount of history.
        if tag == "history" && !batch.is_empty() && snapshot.len() < HISTORY_TARGET {
            if let Some(from) = oldest {
                self.client.send(&json!({
                    "@type": "getChatHistory",
                    "chat_id": chat_id,
                    "from_message_id": from,
                    "offset": 0,
                    "limit": HISTORY_TARGET - snapshot.len(),
                    "only_local": false,
                    "@extra": format!("history:{chat_id}")
                }));
            }
        }

        self.emit(BackendEvent::History { chat_id, messages: snapshot });
    }

    fn on_error(&self, val: &Value) {
        let code = val["code"].as_i64().unwrap_or(0);
        // loadChats reports 404 once the whole list is loaded; not an error.
        if val["@extra"] == "loadChats" && code == 404 {
            return;
        }
        let message = format!("{} ({})", val["message"].as_str().unwrap_or("unknown error"), code);
        if *self.auth_tx.borrow() != AuthState::Ready {
            lock(&self.state).last_error = Some(message);
            // Wake up the login prompt so it can show the error.
            self.auth_tx.send_modify(|_| {});
        } else {
            self.emit(BackendEvent::Error(message));
        }
    }
}

fn formatted_text(text: &str) -> Value {
    json!({ "@type": "formattedText", "text": text })
}

fn chat_id_of(val: &Value) -> i64 {
    val["chat_id"].as_i64().unwrap_or(0)
}

fn parse_order(v: &Value) -> i64 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_i64())
        .unwrap_or(0)
}

/// Apply a single `chatPosition`, ignoring lists other than the main one.
fn apply_position(chat: &mut Chat, pos: &Value) {
    if pos["list"]["@type"] == "chatListMain" {
        chat.order = parse_order(&pos["order"]);
        chat.is_pinned = pos["is_pinned"].as_bool().unwrap_or(false);
    }
}

fn apply_positions(chat: &mut Chat, positions: &Value) {
    if let Some(arr) = positions.as_array() {
        for p in arr {
            apply_position(chat, p);
        }
    }
}

fn build_chat(st: &mut State, c: &Value) -> Chat {
    let id = c["id"].as_i64().unwrap_or(0);
    let mut username = None;
    let chat_type = match c["type"]["@type"].as_str().unwrap_or("") {
        "chatTypePrivate" | "chatTypeSecret" => {
            let user_id = c["type"]["user_id"].as_i64().unwrap_or(0);
            st.user_chats.insert(user_id, id);
            match st.users.get(&user_id) {
                Some(u) => {
                    username = u.username.clone();
                    if u.is_bot {
                        ChatType::Bot
                    } else {
                        ChatType::Direct
                    }
                }
                None => ChatType::Direct,
            }
        }
        "chatTypeSupergroup" if c["type"]["is_channel"].as_bool().unwrap_or(false) => ChatType::Channel,
        _ => ChatType::Group,
    };
    let (preview, time) = last_message_summary(st, id, &c["last_message"]);
    let mut chat = Chat {
        id,
        title: c["title"].as_str().unwrap_or("Unknown").to_string(),
        username,
        chat_type,
        unread_count: c["unread_count"].as_u64().unwrap_or(0) as u32,
        is_pinned: false,
        is_muted: c["notification_settings"]["mute_for"].as_i64().unwrap_or(0) > 0,
        last_message_preview: preview,
        last_message_time: time,
        members_count: None,
        order: 0,
    };
    apply_positions(&mut chat, &c["positions"]);
    chat
}

/// (preview, time) for the chat list.
fn last_message_summary(st: &State, chat_id: i64, m: &Value) -> (String, String) {
    if m.is_null() {
        return (String::new(), String::new());
    }
    let text = content_text(&m["content"]).replace('\n', " ");
    let is_group = st
        .chats
        .get(&chat_id)
        .map(|c| c.chat_type == ChatType::Group)
        .unwrap_or(false);
    let preview = if is_group {
        format!("{}: {}", sender_name(st, m), text)
    } else {
        text
    };
    (preview, format_chat_time(m["date"].as_i64().unwrap_or(0)))
}

fn sender_name(st: &State, m: &Value) -> String {
    if m["is_outgoing"].as_bool().unwrap_or(false) {
        return "Me".into();
    }
    let s = &m["sender_id"];
    match s["@type"].as_str().unwrap_or("") {
        "messageSenderUser" => {
            let uid = s["user_id"].as_i64().unwrap_or(0);
            st.users
                .get(&uid)
                .map(|u| u.name.clone())
                .unwrap_or_else(|| format!("User {uid}"))
        }
        "messageSenderChat" => {
            let cid = s["chat_id"].as_i64().unwrap_or(0);
            st.chats
                .get(&cid)
                .map(|c| c.title.clone())
                .unwrap_or_else(|| "Channel".into())
        }
        _ => "Unknown".into(),
    }
}

/// Human-readable text for any message content type.
fn content_text(content: &Value) -> String {
    let caption = content["caption"]["text"].as_str().unwrap_or("");
    let with_caption = |label: &str| {
        if caption.is_empty() {
            format!("[{label}]")
        } else {
            format!("[{label}] {caption}")
        }
    };
    match content["@type"].as_str().unwrap_or("") {
        "messageText" => content["text"]["text"].as_str().unwrap_or("").to_string(),
        "messagePhoto" => with_caption("Photo"),
        "messageVideo" => with_caption("Video"),
        "messageAnimation" => with_caption("GIF"),
        "messageAudio" => with_caption("Audio"),
        "messageVoiceNote" => with_caption("Voice"),
        "messageVideoNote" => "[Video message]".into(),
        "messageDocument" => {
            let name = content["document"]["file_name"].as_str().unwrap_or("file");
            with_caption(&format!("File: {name}"))
        }
        "messageSticker" => format!("[Sticker {}]", content["sticker"]["emoji"].as_str().unwrap_or("")),
        "messageLocation" => "[Location]".into(),
        "messageContact" => "[Contact]".into(),
        "messagePoll" => format!(
            "[Poll] {}",
            content["poll"]["question"]["text"]
                .as_str()
                .or_else(|| content["poll"]["question"].as_str())
                .unwrap_or("")
        ),
        other => {
            let label = other.strip_prefix("message").unwrap_or(other);
            with_caption(if label.is_empty() { "Message" } else { label })
        }
    }
}

fn file_preview(file: &Value) -> Option<Preview> {
    let file_id = file["id"].as_i64()? as i32;
    let local = &file["local"];
    let local_path = (local["is_downloading_completed"].as_bool() == Some(true))
        .then(|| local["path"].as_str())
        .flatten()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    Some(Preview { file_id, local_path })
}

/// Preview from a `thumbnail` object, if it is a still image we can decode.
fn thumbnail_preview(thumb: &Value) -> Option<Preview> {
    match thumb["format"]["@type"].as_str()? {
        "thumbnailFormatJpeg" | "thumbnailFormatPng" | "thumbnailFormatWebp" => file_preview(&thumb["file"]),
        _ => None,
    }
}

/// Pick a photo size good for inline display: the largest one that is at
/// most 800px on its longer side (TDLib lists sizes smallest first).
fn photo_preview(sizes: &Value) -> Option<Preview> {
    let sizes = sizes.as_array()?;
    let fits = |s: &&Value| s["width"].as_u64().unwrap_or(0).max(s["height"].as_u64().unwrap_or(0)) <= 800;
    let size = sizes.iter().rfind(fits).or_else(|| sizes.first())?;
    file_preview(&size["photo"])
}

/// The downloadable file attached to a message, if any.
fn content_media(content: &Value) -> Option<Media> {
    let (file, kind, preview) = match content["@type"].as_str().unwrap_or("") {
        "messagePhoto" => {
            let sizes = &content["photo"]["sizes"];
            // Largest size is last.
            let full = sizes.as_array()?.last()?["photo"].clone();
            (full, MediaKind::Image, photo_preview(sizes))
        }
        "messageSticker" => {
            let s = &content["sticker"];
            if s["format"]["@type"] == "stickerFormatWebp" {
                (s["sticker"].clone(), MediaKind::Image, file_preview(&s["sticker"]))
            } else {
                (s["sticker"].clone(), MediaKind::File, thumbnail_preview(&s["thumbnail"]))
            }
        }
        "messageDocument" => {
            let d = &content["document"];
            let is_image = d["mime_type"].as_str().unwrap_or("").starts_with("image/");
            let kind = if is_image { MediaKind::Image } else { MediaKind::File };
            let small = d["document"]["size"].as_u64().unwrap_or(u64::MAX) < 10 * 1024 * 1024;
            let preview = thumbnail_preview(&d["thumbnail"]).or_else(|| {
                (is_image && small).then(|| file_preview(&d["document"])).flatten()
            });
            (d["document"].clone(), kind, preview)
        }
        "messageAnimation" => {
            let a = &content["animation"];
            (a["animation"].clone(), MediaKind::File, thumbnail_preview(&a["thumbnail"]))
        }
        "messageVideo" => {
            let v = &content["video"];
            (v["video"].clone(), MediaKind::File, thumbnail_preview(&v["thumbnail"]))
        }
        "messageVideoNote" => {
            let v = &content["video_note"];
            (v["video"].clone(), MediaKind::File, thumbnail_preview(&v["thumbnail"]))
        }
        "messageAudio" => {
            let a = &content["audio"];
            (a["audio"].clone(), MediaKind::File, thumbnail_preview(&a["album_cover_thumbnail"]))
        }
        "messageVoiceNote" => (content["voice_note"]["voice"].clone(), MediaKind::File, None),
        _ => return None,
    };
    let file_id = file["id"].as_i64()? as i32;
    Some(Media { file_id, kind, preview })
}

fn send_state(m: &Value) -> SendState {
    let st = &m["sending_state"];
    match st["@type"].as_str().unwrap_or("") {
        "messageSendingStatePending" => SendState::Pending,
        "messageSendingStateFailed" => SendState::Failed(error_text(&st["error"], st)),
        _ => SendState::Sent,
    }
}

/// Message of an `error` object (falls back to legacy `error_message` fields).
fn error_text(err: &Value, legacy: &Value) -> String {
    err["message"]
        .as_str()
        .or_else(|| legacy["error_message"].as_str())
        .unwrap_or("unknown error")
        .to_string()
}

fn parse_message(st: &State, m: &Value) -> Message {
    let chat_id = m["chat_id"].as_i64().unwrap_or(0);
    let id = m["id"].as_i64().unwrap_or(0);
    let is_outgoing = m["is_outgoing"].as_bool().unwrap_or(false);
    let date = m["date"].as_i64().unwrap_or(0);

    // Newer TDLib: `reply_to: messageReplyToMessage`; older: `reply_to_message_id`.
    let reply_id = if m["reply_to"]["@type"] == "messageReplyToMessage" {
        m["reply_to"]["message_id"].as_i64()
    } else {
        m["reply_to_message_id"].as_i64().filter(|&id| id != 0)
    };
    let reply_to = reply_id.map(|rid| {
        st.messages
            .get(&chat_id)
            .and_then(|list| list.iter().find(|x| x.id == rid))
            .map(|orig| ReplyInfo {
                sender_name: orig.sender_name.clone(),
                text_preview: orig.text.chars().take(40).collect(),
            })
            .unwrap_or_else(|| ReplyInfo {
                sender_name: "Reply".into(),
                text_preview: "(original message not loaded)".into(),
            })
    });

    Message {
        id,
        chat_id,
        sender_name: sender_name(st, m),
        text: content_text(&m["content"]),
        date,
        time_str: format_message_time(date),
        is_outgoing,
        is_read: !is_outgoing || st.outbox_read.get(&chat_id).is_some_and(|&r| id <= r),
        reply_to,
        media: content_media(&m["content"]),
        send_state: send_state(m),
    }
}

/// Insert keeping the list sorted by id. Returns true if the message was new.
fn upsert_message(list: &mut Vec<Message>, msg: Message) -> bool {
    match list.binary_search_by_key(&msg.id, |m| m.id) {
        Ok(i) => {
            list[i] = msg;
            false
        }
        Err(i) => {
            list.insert(i, msg);
            true
        }
    }
}
