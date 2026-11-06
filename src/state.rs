//! UI state saved on exit / chat switch and restored on the next start.

use crate::app::ActivePane;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub chat_id: Option<i64>,
    pub pane: Option<ActivePane>,
    pub selected_message_id: Option<i64>,
    /// Unsent draft and mounted reply.
    #[serde(default)]
    pub input: String,
    pub reply_to: Option<(i64, String)>,
}

/// `$XDG_STATE_HOME/telegram-tui/ui.json`
pub fn path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("telegram-tui")
        .join("ui.json")
}

pub fn load() -> Option<UiState> {
    let data = std::fs::read(path()).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Atomically replace the saved state (errors are not fatal).
pub fn save(state: &UiState) {
    let path = path();
    let Some(dir) = path.parent() else { return };
    let tmp = path.with_extension("json.tmp");
    let result = std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(&tmp, serde_json::to_vec_pretty(state).unwrap_or_default()))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = result {
        log::warn!("failed to save UI state: {e}");
    }
}
