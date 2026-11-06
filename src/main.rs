mod app;
mod backend;
mod config;
mod daemon;
mod event;
mod images;
mod ipc;
mod mode;
mod notify;
mod state;
mod ui;

#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use app::App;
use backend::mock::MockBackend;
use backend::remote::RemoteBackend;
use backend::tdlib::{AuthState, TdlibBackend};
use backend::{Auth, TelegramBackend};
use config::Config;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use event::{handler::handle_event, EventHandler};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Stdout, Write};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

type Term = Terminal<CrosstermBackend<Stdout>>;

const USAGE: &str = "\
telegram-tui — a vim-inspired Telegram TUI client

USAGE:
    telegram-tui [OPTIONS]
    telegram-tui daemon [run|stop|status|install-service] [-c PATH]

The TUI connects to a background service (started automatically) that keeps
Telegram connected and sends desktop notifications after the TUI is closed.

OPTIONS:
    -c, --config <PATH>   Use a custom config.toml
        --chat <ID>       Open this chat on start (used by notifications)
        --no-daemon       Don't use the background service (single process)
        --demo            Run in offline demo mode with mock data
        --single-column   Force single column compact mode
    -h, --help            Show this help
    -v, --version         Show version information

DAEMON:
    run                   Run the service in the foreground (default)
    stop                  Stop the running service
    status                Show whether the service runs and its login state
    install-service       Write a systemd user unit (start with the session)";

#[derive(Default, PartialEq)]
enum DaemonCmd {
    #[default]
    Run,
    Stop,
    Status,
    InstallService,
}

#[derive(Default)]
struct Args {
    config: Option<PathBuf>,
    demo: bool,
    single_column: bool,
    no_daemon: bool,
    chat: Option<i64>,
    daemon: Option<DaemonCmd>,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1).peekable();
    if it.peek().map(String::as_str) == Some("daemon") {
        it.next();
        args.daemon = Some(match it.peek().map(String::as_str) {
            Some("run") => DaemonCmd::Run,
            Some("stop") => DaemonCmd::Stop,
            Some("status") => DaemonCmd::Status,
            Some("install-service") => DaemonCmd::InstallService,
            _ => DaemonCmd::Run,
        });
        if it.peek().is_some_and(|a| !a.starts_with('-')) {
            it.next();
        }
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-v" | "--version" => {
                println!("telegram-tui {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-c" | "--config" => match it.next() {
                Some(p) => args.config = Some(PathBuf::from(p)),
                None => bail!("{arg} requires a path\n\n{USAGE}"),
            },
            "--chat" => match it.next().and_then(|c| c.parse().ok()) {
                Some(id) => args.chat = Some(id),
                None => bail!("--chat requires a numeric chat id\n\n{USAGE}"),
            },
            "--demo" => args.demo = true,
            "--single-column" => args.single_column = true,
            "--no-daemon" => args.no_daemon = true,
            other => match other.strip_prefix("--config=") {
                Some(p) => args.config = Some(PathBuf::from(p)),
                None => bail!("unknown argument: {other}\n\n{USAGE}"),
            },
        }
    }
    Ok(Some(args))
}

fn setup_terminal(mouse: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    Ok(())
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };

    // 1. Load Configuration
    let mut config = Config::load(args.config.as_deref())?;
    if args.single_column {
        config.ui.single_column_breakpoint = u16::MAX;
    }

    if let Some(cmd) = &args.daemon {
        return daemon_command(cmd, &args, config).await;
    }

    // 2. Initialize Backend: background service, in-process TDLib, or mock
    let (backend_tx, backend_rx) = mpsc::unbounded_channel();
    let credentials = config.telegram.clone().filter(|tg| tg.is_configured());
    let telegram_backend: Arc<dyn TelegramBackend> = match credentials {
        Some(tg) if !args.demo => {
            let service = if config.daemon.enabled && !args.no_daemon {
                match connect_service(args.config.as_deref(), backend_tx.clone()).await {
                    Ok(remote) => Some(remote),
                    Err(e) => {
                        eprintln!("Warning: background service unavailable: {e:#}");
                        eprintln!("Running without it (no notifications after exit).");
                        None
                    }
                }
            } else {
                None
            };
            match service {
                Some(remote) => {
                    handle_auth(&*remote).await?;
                    remote
                }
                None => standalone_backend(&tg, backend_tx.clone()).await?,
            }
        }
        _ => {
            if !args.demo {
                println!("No [telegram] api_id/api_hash configured. Running in Mock/Demo mode.");
            }
            Arc::new(MockBackend::new(backend_tx.clone()))
        }
    };

    // 3. Setup Terminal (restored on panic too)
    let default_panic = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        default_panic(panic_info);
    }));
    setup_terminal(config.general.enable_mouse)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let result = run(&mut terminal, telegram_backend, config, args.chat, backend_tx, backend_rx).await;

    // 4. Graceful Teardown
    restore_terminal()?;
    result?;
    println!("telegram-tui exited cleanly. See you!");
    Ok(())
}

/// In-process TDLib (no background service); falls back to demo data.
async fn standalone_backend(
    tg: &config::TelegramCredentials,
    tx: mpsc::UnboundedSender<backend::BackendEvent>,
) -> Result<Arc<dyn TelegramBackend>> {
    println!("Initializing Telegram via TDLib...");
    match TdlibBackend::new(tg.api_id, tg.api_hash.clone(), tg.session_dir(), tx.clone()) {
        Ok(real) => {
            handle_auth(&*real).await?;
            Ok(real)
        }
        Err(e) => {
            eprintln!("\nWarning: Failed to initialize TDLib: {}", e);
            eprintln!("Falling back to MockBackend for offline demonstration.\n");
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(Arc::new(MockBackend::new(tx)))
        }
    }
}

fn daemon_log_path() -> PathBuf {
    state::path().with_file_name("daemon.log")
}

/// Connect to the background service, starting it if it isn't running.
async fn connect_service(
    config_path: Option<&Path>,
    tx: mpsc::UnboundedSender<backend::BackendEvent>,
) -> Result<Arc<RemoteBackend>> {
    let path = ipc::socket_path();
    if let Ok(remote) = RemoteBackend::connect(&path, tx.clone()).await {
        return Ok(remote);
    }

    println!("Starting background service...");
    let log_path = daemon_log_path();
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("daemon");
    if let Some(p) = config_path {
        cmd.arg("--config").arg(std::fs::canonicalize(p)?);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Own session: closing the terminal (SIGHUP) must not take the service down.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("failed to start the background service")?;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(remote) = RemoteBackend::connect(&path, tx.clone()).await {
            // Reap it in the background if it ever exits while we run.
            std::thread::spawn(move || child.wait());
            return Ok(remote);
        }
        if let Ok(Some(status)) = child.try_wait() {
            bail!("service exited ({status}); see {}", log_path.display());
        }
        if Instant::now() > deadline {
            bail!("service did not start in time; see {}", log_path.display());
        }
    }
}

async fn daemon_command(cmd: &DaemonCmd, args: &Args, config: Config) -> Result<()> {
    let path = ipc::socket_path();
    match cmd {
        DaemonCmd::Run => daemon::run(config).await,
        DaemonCmd::Stop => match backend::remote::request_shutdown(&path).await {
            Ok(()) => {
                println!("Background service stopped.");
                Ok(())
            }
            Err(_) => {
                println!("Background service is not running.");
                Ok(())
            }
        },
        DaemonCmd::Status => {
            match backend::remote::query_status(&path).await {
                Ok(ipc::Reply::Status { pid, clients, auth }) => {
                    println!("running: pid {pid}, {clients} TUI client(s), login: {auth:?}");
                    println!("socket:  {}", path.display());
                }
                Ok(_) => println!("running (unexpected status reply)"),
                Err(_) => println!("not running"),
            }
            Ok(())
        }
        DaemonCmd::InstallService => {
            let unit = daemon::install_service(args.config.as_deref())?;
            println!("Wrote {}", unit.display());
            println!("Enable it with:\n    systemctl --user daemon-reload\n    systemctl --user enable --now telegram-tui.service");
            Ok(())
        }
    }
}

async fn run(
    terminal: &mut Term,
    backend: Arc<dyn TelegramBackend>,
    config: Config,
    open_chat: Option<i64>,
    backend_tx: mpsc::UnboundedSender<backend::BackendEvent>,
    backend_rx: mpsc::UnboundedReceiver<backend::BackendEvent>,
) -> Result<()> {
    let mut app = App::new(backend, config, backend_tx);
    app.init().await?;

    // Restore where we left off (or jump to the chat a notification asked for).
    let mut saved = state::load().unwrap_or_default();
    if let Some(chat_id) = open_chat {
        saved = state::UiState {
            chat_id: Some(chat_id),
            pane: Some(app::ActivePane::Messages),
            ..saved
        };
        saved.selected_message_id = None;
    }
    app.restore(saved).await?;

    let mut events = EventHandler::new(backend_rx);

    while !app.should_quit {
        if app.images.enabled {
            app.images.cell_px.set(ui::kitty::cell_size_px());
        }
        terminal.draw(|f| ui::render(f, &app))?;
        app.images.flush(terminal.backend_mut())?;

        let Some(event) = events.rx.recv().await else {
            break;
        };
        handle_event(&mut app, event).await?;
        // Drain whatever else is queued before redrawing (TDLib can deliver
        // hundreds of updates at once during startup).
        while !app.should_quit && !app.editor_requested {
            match events.rx.try_recv() {
                Ok(event) => handle_event(&mut app, event).await?,
                Err(_) => break,
            }
        }

        if app.editor_requested {
            app.editor_requested = false;
            run_editor(terminal, &mut events, &mut app).await?;
        }
    }
    state::save(&app.ui_state());
    let _ = app.backend.set_viewing(None).await;
    app.images.delete_all(terminal.backend_mut())?;
    Ok(())
}

/// Suspend the TUI, run $EDITOR on the input buffer, then send the result.
async fn run_editor(terminal: &mut Term, events: &mut EventHandler, app: &mut App) -> Result<()> {
    events.stop_input().await;
    restore_terminal()?;

    let edited = app.run_editor();

    setup_terminal(app.config.general.enable_mouse)?;
    terminal.clear()?;
    app.images.invalidate();
    events.start_input();

    match edited {
        Ok(Some(text)) => {
            app.input_buffer = text;
            app.send_current_message().await?;
        }
        Ok(None) => app.set_status("Editor closed without sending"),
        Err(e) => app.set_status(format!("Editor failed: {e:#}")),
    }
    Ok(())
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        bail!("stdin closed during login");
    }
    Ok(line.trim().to_string())
}

/// Read a line without echoing it to the terminal.
fn prompt_password(prompt: &str) -> Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};

    print!("{prompt}");
    io::stdout().flush()?;
    enable_raw_mode()?;
    let mut password = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(anyhow::anyhow!("login cancelled"))
                }
                KeyCode::Char(c) => password.push(c),
                KeyCode::Backspace => {
                    password.pop();
                }
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
    };
    disable_raw_mode()?;
    println!();
    result.map(|_| password)
}

async fn handle_auth(backend: &dyn Auth) -> Result<()> {
    let mut auth = backend.auth_watch();
    let mut announced_connecting = false;

    loop {
        let state = auth.borrow_and_update().clone();
        if let Some(err) = backend.take_error().await {
            eprintln!("[Telegram Login] Error: {err}");
            if state == AuthState::WaitingParams {
                bail!("TDLib rejected the parameters (check api_id / api_hash): {err}");
            }
        }

        match state {
            AuthState::WaitingParams => {
                if !announced_connecting {
                    println!("[Telegram Login] Connecting to Telegram...");
                    announced_connecting = true;
                }
            }
            AuthState::WaitingPhone => {
                let phone = prompt_line("\n[Telegram Login] Enter your phone number (e.g. +8613800000000): ")?;
                backend.send_phone_number(&phone).await?;
            }
            AuthState::WaitingCode => {
                let code = prompt_line("[Telegram Login] Enter authentication code sent to your Telegram app: ")?;
                backend.send_auth_code(&code).await?;
            }
            AuthState::WaitingPassword => {
                let password = prompt_password("[Telegram Login] Enter your 2FA Cloud Password: ")?;
                backend.send_password(&password).await?;
            }
            AuthState::Ready => {
                println!("[Telegram Login] Authenticated successfully! Launching TUI...");
                return Ok(());
            }
            AuthState::Closed => bail!("Telegram client closed during login"),
            AuthState::Other(s) => {
                eprintln!("[Telegram Login] Unsupported login step `{s}`; waiting...");
            }
        }

        // Wait for the next state change (or an error notification).
        auth.changed().await?;
    }
}
