use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub wayland: WaylandConfig,
    #[serde(default)]
    pub telegram: Option<TelegramCredentials>,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    /// The TUI connects to (and auto-starts) the background service, which
    /// keeps Telegram connected and sends notifications after the TUI exits.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Run when a notification is clicked and no TUI is open.
    /// `{exe}` = this binary, `{chat_id}` = the chat to open.
    #[serde(default = "default_open_tui")]
    pub open_command: String,
    /// Icon name (from the icon theme) or path for notifications.
    #[serde(default = "default_notify_icon")]
    pub notify_icon: String,
    /// Offer "Reply" inside notifications when the server supports it.
    #[serde(default = "default_true")]
    pub inline_reply: bool,
    /// On niri, focus the TUI window when a notification is clicked.
    #[serde(default = "default_true")]
    pub focus_window: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            open_command: default_open_tui(),
            notify_icon: default_notify_icon(),
            inline_reply: true,
            focus_window: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_editor")]
    pub editor: String,
    #[serde(default = "default_true")]
    pub auto_mark_read: bool,
    #[serde(default = "default_true")]
    pub enable_mouse: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiConfig {
    /// true: inherit the terminal's ANSI palette; false: built-in truecolor theme.
    #[serde(default = "default_true")]
    pub use_terminal_colors: bool,
    #[serde(default = "default_true")]
    pub rounded_borders: bool,
    #[serde(default = "default_breakpoint")]
    pub single_column_breakpoint: u16,
    #[serde(default = "default_ratio")]
    pub chats_ratio_percent: u16,
    /// Use Nerd Font glyphs for icons; false = plain Unicode symbols.
    #[serde(default = "default_true")]
    pub nerd_font: bool,
    /// Show photos inline (kitty graphics protocol; ignored on other terminals).
    #[serde(default = "default_true")]
    pub inline_images: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WaylandConfig {
    #[serde(default = "default_copy")]
    pub copy_command: String,
    #[serde(default = "default_paste")]
    pub paste_command: String,
    /// Opens photos / image stickers (`o`).
    #[serde(default = "default_viewer")]
    pub image_viewer: String,
    /// Opens all other media: video, audio, documents (`o`).
    #[serde(default = "default_open")]
    pub open_command: String,
    #[serde(default = "default_notify")]
    pub notify_command: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramCredentials {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default = "default_session_path")]
    pub session_path: String,
}

fn default_editor() -> String {
    std::env::var("EDITOR").unwrap_or_else(|_| "nvim".into())
}
fn default_true() -> bool {
    true
}
fn default_breakpoint() -> u16 {
    80
}
fn default_ratio() -> u16 {
    28
}
fn default_copy() -> String {
    "wl-copy".into()
}
fn default_paste() -> String {
    "wl-paste".into()
}
fn default_viewer() -> String {
    "imv".into()
}
fn default_open() -> String {
    "xdg-open".into()
}
fn default_open_tui() -> String {
    "kitty --class telegram-tui -e {exe} --chat {chat_id}".into()
}
fn default_notify_icon() -> String {
    "org.telegram.desktop".into()
}
fn default_notify() -> String {
    "notify-send".into()
}
fn default_session_path() -> String {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("telegram-tui")
        .join("tdlib")
        .to_string_lossy()
        .to_string()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            editor: default_editor(),
            auto_mark_read: true,
            enable_mouse: true,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            use_terminal_colors: true,
            rounded_borders: true,
            single_column_breakpoint: default_breakpoint(),
            chats_ratio_percent: default_ratio(),
            nerd_font: true,
            inline_images: true,
        }
    }
}

impl Default for WaylandConfig {
    fn default() -> Self {
        Self {
            copy_command: default_copy(),
            paste_command: default_paste(),
            image_viewer: default_viewer(),
            open_command: default_open(),
            notify_command: default_notify(),
        }
    }
}

impl TelegramCredentials {
    /// `api_id = 0` / empty hash (as in config.example.toml) means "not configured".
    pub fn is_configured(&self) -> bool {
        self.api_id != 0 && !self.api_hash.trim().is_empty()
    }

    pub fn session_dir(&self) -> PathBuf {
        expand_tilde(&self.session_path)
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(rest),
        None if path == "~" => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        None => PathBuf::from(path),
    }
}

impl Config {
    /// Load from an explicit path (errors are fatal), or search
    /// `./config.toml` then `~/.config/telegram-tui/config.toml`.
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            let cfg = Self::try_read_config(path)
                .with_context(|| format!("Failed to load config {}", path.display()))?;
            println!("[Config] Loaded configuration from {}", path.display());
            return Ok(cfg);
        }

        let global_path = Self::config_path();
        for path in [PathBuf::from("config.toml"), global_path.clone()] {
            if !path.exists() {
                continue;
            }
            match Self::try_read_config(&path) {
                Ok(cfg) => {
                    println!("[Config] Loaded configuration from {}", path.display());
                    return Ok(cfg);
                }
                Err(e) => eprintln!("[Config Warning] Failed parsing {}: {}", path.display(), e),
            }
        }

        eprintln!(
            "[Config Info] No config file found at {} or ./config.toml. Using defaults.",
            global_path.display()
        );
        Ok(Self::default())
    }

    fn try_read_config(path: &Path) -> Result<Config> {
        let content = fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&content)?;
        cfg.ui.chats_ratio_percent = cfg.ui.chats_ratio_percent.clamp(10, 90);
        Ok(cfg)
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("telegram-tui")
            .join("config.toml")
    }
}
