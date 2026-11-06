# telegram-tui

A modern, minimalist, vim-inspired Telegram TUI client built in Rust with Ratatui, specifically crafted for **Arch Linux + niri (scrollable-tiling Wayland) + fish + noctalia**.

---

## ✨ Features

- **🌐 Real Telegram MTProto Support via TDLib**: Connects to official Telegram servers via Arch Linux's official `libtdjson.so` (`sudo pacman -S tdlib`).
- **🎨 Terminal 16-Color Pure & Transparent**: Zero hardcoded hex colors. Inherits your terminal palette (ANSI 16 colors) and background transparency (seamlessly matching pywal, base16, and Noctalia themes).
- **📐 Responsive niri Column Mode**: Automatically adapts between dual-column view (Chats + Messages) and single-column compact view when niri columns scale below 80 characters.
- **⚡ Vim-Centric Modal UX**:
  - `NORMAL` mode: `j`/`k` message navigation, `r` instant quote reply, `y` yank to Wayland clipboard, `e` launch Neovim.
  - `INSERT` mode: Fast typing with suspended quote banner and dynamic prompt hints.
  - `FUZZY` finder: Telescope-style floating popup (`/` or `<Space>f`) to jump between dialogs in real time.
- **💬 Chat Bubbles**: Every message is drawn in a bordered bubble — your own on the right, others on the left.
- **🖼️ Inline Images (kitty)**: Photos, stickers and video/file thumbnails are shown right inside the bubbles via the kitty graphics protocol (Unicode placeholders, so they scroll with the text). Other terminals fall back to `[Photo]` labels.
- **🐧 Pure Wayland Integration**:
  - `wl-copy` & `wl-paste` native hooks.
  - Media opening via lightweight Wayland tools (`imv`, `swayimg`, `mpv`); falls back to `xdg-open`.
  - Paste screenshots / copied files straight into a chat with `<C-v>`.
- **🛡️ Crash-Resilient**: Embedded panic hooks ensure terminal raw mode and alternate screen are always cleanly restored.

---

## 🚀 Connecting to Real Telegram (Arch Linux)

### 1. Install System TDLib
```fish
sudo pacman -S tdlib
```

### 2. Configure Your API Credentials
Get your `api_id` and `api_hash` from [https://my.telegram.org](https://my.telegram.org) under "API development tools".

Edit `~/.config/telegram-tui/config.toml`:
```toml
[telegram]
api_id = 12345678
api_hash = "your_32_character_api_hash_here"
session_path = "~/.local/share/telegram-tui/tdlib"
```

### 3. Launch and Authenticate
```fish
cargo run --release
```
On the first run, the client will prompt for your phone number, the verification code sent to your Telegram app, and your 2FA password (if enabled). After logging in once, your session is saved locally in SQLite and all subsequent starts will connect instantly!

---

## ⌨️ Vim Keybindings

### NORMAL Mode (Navigation & Control)

| Key | Action |
| :--- | :--- |
| `j` / `k` | Scroll down / up messages (or move through chat list); `k` at the top loads older history |
| `h` / `l` / `<Enter>` | Switch pane focus between **Chats** and **Messages** (focusing messages marks the chat read) |
| `G` / `gg` | Jump to newest / oldest message in history |
| `r` | **Reply** to highlighted message (mounts quote card and enters INSERT) |
| `y` | **Yank** (copy) message text directly to Wayland clipboard via `wl-copy` |
| `o` | **Open** the photo/file of the selected message (`image_viewer` for images, `open_command` otherwise) |
| `d` | Delete selected message (with confirmation prompt) |
| `i` / `a` | Enter **INSERT** mode to type a message |
| `e` | Open **`$EDITOR`** (Neovim) to write long/Markdown messages; sends on `:wq`, `:cq` aborts |
| `/` or `<Space>f` | Open **Fuzzy Finder** modal (Telescope style) |
| `?` | Toggle Keybinding Help Cheat Sheet |
| `q` / `<C-c>` | Quit client cleanly |

### INSERT Mode (Typing & Dispatch)

| Key | Action |
| :--- | :--- |
| `<Enter>` | Send message and return to NORMAL mode |
| `<Esc>` | Exit to NORMAL mode (cancels mounted reply and attachments) |
| `<C-e>` | Pop open Neovim with current input; sends upon `:wq` |
| `<C-v>` | Paste from clipboard: images (screenshots) and files copied in a file manager become attachments sent on `<Enter>` (input text = caption); otherwise pastes text |
| `<C-u>` | Clear current input buffer |
| `<C-w>` | Delete word backwards |

### FUZZY Finder Mode (Popup)

| Key | Action |
| :--- | :--- |
| `<Enter>` | Jump to selected chat/group/channel |
| `<C-j>` / `<Down>` | Select next search result |
| `<C-k>` / `<Up>` | Select previous search result |
| `<Esc>` | Dismiss popup |

---

## 🖥️ Command Line

```fish
telegram-tui [-c|--config PATH] [--demo] [--single-column] [-h|--help] [-v|--version]
```

Config is read from `--config`, else `./config.toml`, else `~/.config/telegram-tui/config.toml`.
TDLib logs are written to `<session_path>/tdlib.log`. Desktop notifications (`notify_command`)
are sent for new messages in other, non-muted chats.

---

## ⚙️ niri Integration (`~/.config/niri/config.kdl`)

```kdl
binds {
    Mod+T { spawn "alacritty" "--class" "telegram-tui" "-e" "telegram-tui"; }
}

window-rule {
    match app-id="telegram-tui"
    default-column-width { proportion 0.45; }
}
```
