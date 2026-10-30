//! Terminal frontend for SHCCL.
//!
//! The TUI exposes the same features as the CLI and SDL GUI: dashboard,
//! versions (installed and remote), offline accounts, settings and a live log
//! view fed by install progress, launch previews and game stdout/stderr.

mod app;
mod ui;

use std::io::{self, Stdout};
use std::panic;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use shccl_core::Launcher;

use crate::app::App;

fn main() -> Result<()> {
    let root = parse_root()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let launcher = {
        let _guard = runtime.enter();
        Launcher::new(root)?
    };
    let mut app = App::new(launcher, runtime.handle().clone());

    install_panic_hook();
    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        app.poll_backend();
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.quit {
            break;
        }

        // Short polling keeps the UI responsive while background install and
        // launch tasks stream events.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse.kind, mouse.column, mouse.row),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Make sure a panic does not leave the user's terminal in raw/alternate mode.
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(info);
    }));
}

fn parse_root() -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--root" {
            return match args.next() {
                Some(value) => Ok(Some(PathBuf::from(value))),
                None => anyhow::bail!("--root requires a directory argument"),
            };
        }
        if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--root=")) {
            return Ok(Some(PathBuf::from(value)));
        }
        if arg == "--help" || arg == "-h" {
            println!(
                "SHCCL - Smart Home Control Craft Launcher (TUI)\n\n\
                 Usage: shccl-tui [--root DIR]\n\n\
                 --root DIR   Launcher root (defaults to SHCCL_ROOT or the platform data directory)\n\n\
                 Keys: 1-5 switch pages, r refresh, ? help, q/Esc quit."
            );
            std::process::exit(0);
        }
        if arg == "--version" || arg == "-V" {
            println!("shccl-tui {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }
    Ok(None)
}
