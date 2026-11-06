# tgtui

A minimalist, vim-inspired Telegram client for the terminal, written in Rust with
[Ratatui](https://ratatui.rs) on top of the official [TDLib](https://core.telegram.org/tdlib).
Built for **Arch Linux + niri (scrollable-tiling Wayland) + kitty + Noctalia**, but works in any
terminal.

The crate and binary are called `telegram-tui`.

---

## Features

- **Real Telegram account** via TDLib (`libtdjson.so`, loaded at runtime): chats, history,
  replies, edits, deletions, read receipts (`✓` delivered / `✓✓` read), send failures shown inline.
- **Vim-style modal UI**: `NORMAL`, `INSERT` and a Telescope-like fuzzy chat finder.
- **Chat bubbles**: bordered messages, yours on the right and everyone else's on the left.
- **Inline images in kitty**: photos, stickers and video/file thumbnails are drawn inside the
  bubbles with the kitty graphics protocol (Unicode placeholders, so they scroll with the text).
  Other terminals show `[Photo]` labels instead.
- **Media**: paste screenshots or copied files with `<C-v>`, yank images/files with `y`, open
  them with `o`.
- **Background service**: keeps Telegram connected after you close the TUI and sends desktop
  notifications (one per chat, with *Open*, *Mark as read* and inline *Reply*). Reopening the
  TUI restores where you left off.
- **Fits your desktop**: inherits the terminal's 16-color palette and transparency (pywal,
  base16, Noctalia), uses Nerd Font icons, switches to a single column on narrow niri columns.

---

## Requirements

- Linux with a recent Rust toolchain (`cargo`) to build.
- **TDLib** (`libtdjson.so`). It is looked up, in order, at
  `~/.local/share/telegram-tui/tdlib-lib/libtdjson.so`, `/usr/lib/libtdjson.so`, then the
  dynamic loader path. On Arch it is available from the AUR (e.g. `telegram-tdlib`), or build
  it from [source](https://github.com/tdlib/td) and copy `libtdjson.so` to the first path.
- A Telegram `api_id` / `api_hash` from <https://my.telegram.org> ("API development tools").
- Optional: `wl-clipboard` (`wl-copy` / `wl-paste`), a Nerd Font (icons), kitty (inline
  images), a notification server such as Noctalia (notifications), `imv` / `xdg-open` (`o`).

Without credentials (or with `--demo`) the client runs in an offline demo mode with mock chats.

---

## Install

```fish
git clone https://github.com/Eudocia6283/tgtui
cd tgtui
cargo install --path .          # installs ~/.cargo/bin/telegram-tui

mkdir -p ~/.config/telegram-tui
cp config.example.toml ~/.config/telegram-tui/config.toml
cp completions/telegram-tui.fish ~/.config/fish/completions/   # optional
```

Then set your credentials in `~/.config/telegram-tui/config.toml`:

```toml
[telegram]
api_id = 12345678
api_hash = "your_32_character_api_hash_here"
session_path = "~/.local/share/telegram-tui/tdlib"
```

Run `telegram-tui`. The first start asks for your phone number, the login code and your 2FA
password (if any); the session is then stored under `session_path` and later starts connect
right away. Try the UI without an account with `telegram-tui --demo`.

---

## Keybindings

### NORMAL Mode (Navigation & Control)

| Key | Action |
| :--- | :--- |
| `j` / `k` | Scroll down / up messages (or move through chat list); `k` at the top loads older history |
| `h` / `l` / `<Enter>` | Switch pane focus between **Chats** and **Messages** (focusing messages marks the chat read) |
| `G` / `gg` | Jump to newest / oldest message in history |
| `r` | **Reply** to highlighted message (mounts quote card and enters INSERT) |
| `y` | **Yank** to the Wayland clipboard via `wl-copy`: images as PNG data, other files as `text/uri-list` (pastes into file managers / back into a chat), text otherwise |
| `Y` | Yank only the text (for media messages: the caption) |
| `o` | **Open** the photo/file of the selected message (`image_viewer` for images, `open_command` otherwise) |
| `d` | Delete selected message (with confirmation prompt) |
| `i` / `a` | Enter **INSERT** mode to type a message |
| `e` | Compose in **`$EDITOR`** (`general.editor`); sends on `:wq`, `:cq` aborts |
| `/` or `<Space>f` | Open **Fuzzy Finder** modal (Telescope style) |
| `?` | Toggle Keybinding Help Cheat Sheet |
| `q` / `<C-c>` | Quit client cleanly |

### INSERT Mode (Typing & Dispatch)

| Key | Action |
| :--- | :--- |
| `<Enter>` | Send message and return to NORMAL mode |
| `<Esc>` | Exit to NORMAL mode (cancels mounted reply and attachments) |
| `<C-e>` | Continue the current input in `$EDITOR`; sends upon `:wq` |
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

## Background Service & Notifications

The TUI is a client of a small background service (`telegram-tui daemon`) that owns the
Telegram (TDLib) connection. It is started automatically the first time you open the TUI and
keeps running after you quit (`q`) or close the terminal, so you keep getting notifications.

- Notifications go straight to the desktop notification server over D-Bus (Noctalia's
  `org.freedesktop.Notifications`): one per chat, updated as messages arrive, closed once the
  chat is read anywhere.
- Actions: click / **Open** jumps to the chat (in the running TUI — focusing its niri window — or
  in a new `kitty` window), **Mark as read**, and **Reply** right from the notification
  (inline reply).
- Chats you are looking at in a focused TUI window don't notify; muted chats never do. Your
  account only shows as *online* while a TUI window has focus.
- Reopening the TUI restores the chat, pane, selected message, draft and mounted reply.

```fish
telegram-tui daemon status            # running? logged in?
telegram-tui daemon stop              # stop it (the next TUI start brings it back)
telegram-tui daemon install-service   # optional: start with the graphical session (systemd --user)
telegram-tui --no-daemon              # old single-process mode
```

Service log (when auto-started): `~/.local/state/telegram-tui/daemon.log`.

---

## Command Line

```fish
telegram-tui [-c|--config PATH] [--chat ID] [--no-daemon] [--demo] [--single-column] [-h|--help] [-v|--version]
telegram-tui daemon [run|stop|status|install-service] [-c PATH]
```

Config is read from `--config`, else `./config.toml`, else `~/.config/telegram-tui/config.toml`.
TDLib logs are written to `<session_path>/tdlib.log`.

---

## niri Integration (`~/.config/niri/config.kdl`)

```kdl
binds {
    Mod+T { spawn "kitty" "--class" "telegram-tui" "-e" "telegram-tui"; }
}

window-rule {
    match app-id="telegram-tui"
    default-column-width { proportion 0.45; }
}
```

Notifications that open a new window use `daemon.open_command` (by default
`kitty --class telegram-tui -e {exe} --chat {chat_id}`), so the same window rule applies.

---

## Configuration

All options with their defaults are documented in [`config.example.toml`](config.example.toml).

| Path | Contents |
| :--- | :--- |
| `~/.config/telegram-tui/config.toml` | Configuration (`./config.toml` and `--config` take precedence) |
| `~/.local/share/telegram-tui/tdlib/` | TDLib session database and downloaded files (`session_path`) |
| `~/.local/state/telegram-tui/` | Saved UI state (`ui.json`) and the service log (`daemon.log`) |
| `$XDG_RUNTIME_DIR/telegram-tui.sock` | Socket between the TUI and the background service |

---

## Development

```fish
cargo test                      # unit + end-to-end UI tests (mock backend, no account needed)
cargo clippy --all-targets
cargo fmt
cargo run -- --demo             # try the UI with mock data
cargo test live_dbus -- --ignored   # sends one real desktop notification
```

---

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
