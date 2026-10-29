use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use shccl_core::{AccountStore, InstalledVersionSummary, Launcher};

struct App {
    launcher: Launcher,
    installed: Vec<InstalledVersionSummary>,
    accounts: AccountStore,
    selected: usize,
    status: String,
}

impl App {
    fn new(launcher: Launcher) -> Self {
        let mut app = Self {
            launcher,
            installed: Vec::new(),
            accounts: AccountStore::default(),
            selected: 0,
            status: String::new(),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.accounts = self.launcher.accounts();
        match self.launcher.installed_versions() {
            Ok(installed) => {
                self.installed = installed;
                if self.selected >= self.installed.len() {
                    self.selected = self.installed.len().saturating_sub(1);
                }
                self.status = format!(
                    "Loaded {} installed versions and {} accounts.",
                    self.installed.len(),
                    self.accounts.accounts.len()
                );
            }
            Err(error) => {
                self.installed.clear();
                self.status = format!("Could not read installed versions: {error}");
            }
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(4),
            ])
            .split(frame.area());

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                "SHCCL",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Smart Home Control Craft Launcher"),
        ]))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(title, chunks[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(chunks[1]);

        let account_name = self
            .accounts
            .current()
            .map(|account| format!("{} ({})", account.username, account.uuid))
            .unwrap_or_else(|| "none - run `shccl login offline <name>`".to_string());

        let info = vec![
            Line::from(vec![
                Span::styled("Root: ", Style::default().fg(Color::Yellow)),
                Span::raw(self.launcher.paths().root().display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("Java: ", Style::default().fg(Color::Yellow)),
                Span::raw(format!("{:?}", self.launcher.config().java_path)),
            ]),
            Line::from(vec![
                Span::styled("Memory: ", Style::default().fg(Color::Yellow)),
                Span::raw(format!("{} MiB", self.launcher.config().memory_mb)),
            ]),
            Line::from(vec![
                Span::styled("Account: ", Style::default().fg(Color::Yellow)),
                Span::raw(account_name),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "This is the CLI-first scaffold.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from("Use `shccl` to install, log in and launch."),
        ];
        frame.render_widget(
            Paragraph::new(info)
                .wrap(Wrap { trim: false })
                .block(Block::default().title(" Status ").borders(Borders::ALL)),
            body[0],
        );

        let items = if self.installed.is_empty() {
            vec![ListItem::new("No installed versions")]
        } else {
            self.installed
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    let style = if index == self.selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Green)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(format!(
                        "{:<22} {:<10} {}",
                        version.id,
                        version.version_type,
                        version.main_class.as_deref().unwrap_or("-")
                    ))
                    .style(style)
                })
                .collect()
        };
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .title(" Installed versions ")
                    .borders(Borders::ALL),
            ),
            body[1],
        );

        let footer = Paragraph::new(vec![
            Line::from(self.status.as_str()),
            Line::from(Span::styled(
                "j/k or arrows: move   r: refresh   q: quit",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }
}

fn main() -> Result<()> {
    let root = parse_root();
    let launcher = Launcher::new(root)?;
    let mut app = App::new(launcher);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('r') => app.refresh(),
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.installed.is_empty() {
                    app.selected = (app.selected + 1).min(app.installed.len() - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.selected = app.selected.saturating_sub(1);
            }
            KeyCode::Home => app.selected = 0,
            KeyCode::End => app.selected = app.installed.len().saturating_sub(1),
            _ => {}
        }
    }
}

fn parse_root() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--root" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}
