mod app;
mod backend;
mod config;
mod event;
mod mode;
mod ui;

#[cfg(test)]
mod tests;

use anyhow::{bail, Result};
use app::App;
use backend::mock::MockBackend;
use backend::tdlib::{AuthState, TdlibBackend};
use backend::TelegramBackend;
use config::Config;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use event::{handler::handle_event, EventHandler};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Stdout, Write};
use std::panic;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

type Term = Terminal<CrosstermBackend<Stdout>>;

const USAGE: &str = "\
telegram-tui — a vim-inspired Telegram TUI client

USAGE:
    telegram-tui [OPTIONS]

OPTIONS:
    -c, --config <PATH>   Use a custom config.toml
        --demo            Run in offline demo mode with mock data
        --single-column   Force single column compact mode
    -h, --help            Show this help
    -v, --version         Show version information";

#[derive(Default)]
struct Args {
    config: Option<PathBuf>,
    demo: bool,
    single_column: bool,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
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
            "--demo" => args.demo = true,
            "--single-column" => args.single_column = true,
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
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
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

    // 2. Initialize Backend (Real TDLib if configured, or Mock fallback)
    let (backend_tx, backend_rx) = mpsc::unbounded_channel();
    let credentials = config.telegram.as_ref().filter(|tg| tg.is_configured());
    let telegram_backend: Arc<dyn TelegramBackend> = match credentials {
        Some(tg) if !args.demo => {
            println!("Initializing Telegram via TDLib...");
            match TdlibBackend::new(tg.api_id, tg.api_hash.clone(), tg.session_dir(), backend_tx.clone()) {
                Ok(real_backend) => {
                    // Interactive authentication prompt if not already logged in
                    handle_auth(&real_backend).await?;
                    real_backend
                }
                Err(e) => {
                    eprintln!("\nWarning: Failed to initialize TDLib: {}", e);
                    eprintln!("Falling back to MockBackend for offline demonstration.\n");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    Arc::new(MockBackend::new(backend_tx.clone()))
                }
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

    let result = run(&mut terminal, telegram_backend, config, backend_tx, backend_rx).await;

    // 4. Graceful Teardown
    restore_terminal()?;
    result?;
    println!("telegram-tui exited cleanly. See you!");
    Ok(())
}

async fn run(
    terminal: &mut Term,
    backend: Arc<dyn TelegramBackend>,
    config: Config,
    backend_tx: mpsc::UnboundedSender<backend::BackendEvent>,
    backend_rx: mpsc::UnboundedReceiver<backend::BackendEvent>,
) -> Result<()> {
    let mut app = App::new(backend, config, backend_tx);
    app.init().await?;

    let mut events = EventHandler::new(backend_rx);

    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &app))?;

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
    Ok(())
}

/// Suspend the TUI, run $EDITOR on the input buffer, then send the result.
async fn run_editor(terminal: &mut Term, events: &mut EventHandler, app: &mut App) -> Result<()> {
    events.stop_input().await;
    restore_terminal()?;

    let edited = app.run_editor();

    setup_terminal(app.config.general.enable_mouse)?;
    terminal.clear()?;
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

async fn handle_auth(backend: &Arc<TdlibBackend>) -> Result<()> {
    let mut auth = backend.auth_watch();
    let mut announced_connecting = false;

    loop {
        let state = auth.borrow_and_update().clone();
        if let Some(err) = backend.take_error() {
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
                backend.send_phone_number(&phone);
            }
            AuthState::WaitingCode => {
                let code = prompt_line("[Telegram Login] Enter authentication code sent to your Telegram app: ")?;
                backend.send_auth_code(&code);
            }
            AuthState::WaitingPassword => {
                let password = prompt_password("[Telegram Login] Enter your 2FA Cloud Password: ")?;
                backend.send_password(&password);
            }
            AuthState::Ready => {
                println!("[Telegram Login] Authenticated successfully! Launching TUI...");
                return Ok(());
            }
            AuthState::Closed => bail!("TDLib client closed during login"),
            AuthState::Other(s) => {
                eprintln!("[Telegram Login] Unsupported login step `{s}`; waiting...");
            }
        }

        // Wait for the next state change (or an error notification).
        auth.changed().await?;
    }
}
