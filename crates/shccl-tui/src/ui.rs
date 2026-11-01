//! Rendering for the SHCCL terminal UI.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::app::{App, InputModal, Level, Page, RowTarget, Setting, VersionTab};

const ACCENT: Color = Color::Green;
const LABEL: Color = Color::Yellow;
const MUTED: Color = Color::DarkGray;
const INFO: Color = Color::Cyan;
const ERROR: Color = Color::Red;

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    app.tab_hits.clear();
    app.version_tab_hits.clear();
    app.row_hits.clear();
    app.log_area = None;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    match app.page {
        Page::Dashboard => draw_dashboard(frame, app, chunks[1]),
        Page::Versions => draw_versions(frame, app, chunks[1]),
        Page::Accounts => draw_accounts(frame, app, chunks[1]),
        Page::Settings => draw_settings(frame, app, chunks[1]),
        Page::Logs => draw_logs(frame, app, chunks[1]),
    }
    draw_footer(frame, app, chunks[2]);

    if app.help {
        draw_help(frame);
    }
    if app.modal.is_some() {
        draw_modal(frame, app);
    }
    if app.microsoft.is_some() {
        draw_microsoft(frame, app);
    }
}

fn page_block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .title(title.into())
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .title(Line::from(vec![
            Span::styled(
                " SHCCL ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Smart Home Control Craft Launcher ",
                Style::default().fg(Color::Gray),
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let mut spans = Vec::new();
    let mut x = inner.x;
    for (index, page) in Page::ALL.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let label = format!(" {} {} ", page.hotkey(), page.title());
        let width = label.chars().count() as u16;
        let style = if *page == app.page {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        app.tab_hits.push((Rect::new(x, inner.y, width, 1), *page));
        x = x.saturating_add(width);
    }

    // Account and running indicators on the right when there is room. The
    // padding is part of the same line so it cannot overpaint the tabs.
    let mut right = Vec::new();
    if let Some(account) = app.current_account_name() {
        right.push(Span::styled(
            format!("account: {account} "),
            Style::default().fg(LABEL),
        ));
    }
    if let Some(version) = &app.running {
        right.push(Span::styled(
            format!("running: {version} "),
            Style::default().fg(ACCENT),
        ));
    }
    if !right.is_empty() {
        let right_width: u16 = right
            .iter()
            .map(|span| span.content.chars().count() as u16)
            .sum();
        let available = inner.x.saturating_add(inner.width);
        if x.saturating_add(right_width) < available {
            let padding = available.saturating_sub(x).saturating_sub(right_width);
            spans.push(Span::raw(" ".repeat(padding as usize)));
            spans.extend(right);
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_dashboard(frame: &mut Frame, app: &App, area: Rect) {
    let block = page_block(" Dashboard ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let config = &app.config;
    let mut lines = Vec::new();
    lines.push(field(
        "Root",
        app.launcher.paths().root().display().to_string(),
    ));
    lines.push(field(
        "Config file",
        app.launcher.paths().config_file().display().to_string(),
    ));
    lines.push(field(
        "Accounts file",
        app.launcher.paths().accounts_file().display().to_string(),
    ));
    match &app.java {
        Some(Ok(display)) => lines.push(field_styled("Java", display.clone(), ACCENT)),
        Some(Err(error)) => lines.push(field_styled(
            "Java",
            format!("unavailable ({error})"),
            ERROR,
        )),
        None => lines.push(field_styled("Java", "detecting...".to_string(), MUTED)),
    }
    lines.push(field(
        "Profiles",
        format!(
            "{} known · selected {}",
            app.accounts.accounts.len(),
            app.current_account_name().unwrap_or("none")
        ),
    ));
    lines.push(field("Installed versions", app.installed.len().to_string()));
    lines.push(field("Memory", format!("{} MiB", config.memory_mb)));
    lines.push(field(
        "Default version",
        config
            .default_version
            .clone()
            .unwrap_or_else(|| "none".to_string()),
    ));
    lines.push(field(
        "Game directory",
        config
            .game_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(per-version directory)".to_string()),
    ));
    lines.push(field(
        "Download concurrency",
        config.download_concurrency.to_string(),
    ));
    match &app.manifest {
        Some(manifest) => lines.push(field(
            "Remote manifest",
            format!(
                "{} versions · latest release {} · latest snapshot {}",
                manifest.versions.len(),
                manifest.latest.release,
                manifest.latest.snapshot
            ),
        )),
        None => lines.push(field_styled(
            "Remote manifest",
            "not loaded (press 2, then l)".to_string(),
            MUTED,
        )),
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter or r refreshes this page and re-detects Java. Use 2 versions · 3 accounts · 4 settings · 5 logs.",
        Style::default().fg(MUTED),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default()),
        inner,
    );
}

fn draw_versions(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = page_block(" Versions ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    // Tab bar.
    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = inner.x;
    let remote_total = app
        .manifest
        .as_ref()
        .map(|manifest| manifest.versions.len())
        .unwrap_or(0);
    let tabs = [
        (
            VersionTab::Installed,
            format!(" Installed ({}) ", app.installed.len()),
        ),
        (VersionTab::Remote, format!(" Remote ({remote_total}) ")),
    ];
    for (index, (tab, label)) in tabs.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let width = label.chars().count() as u16;
        let style = if app.version_tab == tab {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        hits.push((Rect::new(x, chunks[0].y, width, 1), tab));
        x = x.saturating_add(width);
    }
    if !app.remote_filter.is_empty() {
        spans.push(Span::styled(
            format!("  filter: {} ", app.remote_filter),
            Style::default().fg(LABEL),
        ));
    }
    if let Some(manifest) = &app.manifest {
        spans.push(Span::styled(
            format!("  latest: {} ", manifest.latest.release),
            Style::default().fg(MUTED),
        ));
    }
    app.version_tab_hits = hits;
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    let rows = chunks[2].height as usize;
    app.view_rows = rows.max(1);

    match app.version_tab {
        VersionTab::Installed => {
            let header = Line::from(Span::styled(
                format!("  {:<22} {:<10} {}", "ID", "TYPE", "MAIN CLASS"),
                Style::default().fg(MUTED),
            ));
            frame.render_widget(Paragraph::new(header), chunks[1]);

            if app.installed.is_empty() {
                app.installed_state.select(None);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No installed versions yet. Press l to load the remote manifest, then i to install.",
                        Style::default().fg(MUTED),
                    ))),
                    chunks[2],
                );
                return;
            }
            if rows == 0 {
                return;
            }

            let default_version = app.config.default_version.clone();
            let items: Vec<ListItem> = app
                .installed
                .iter()
                .map(|version| {
                    let marker = if default_version.as_deref() == Some(version.id.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    ListItem::new(format!(
                        "{marker} {:<22} {:<10} {}",
                        version.id,
                        version.version_type,
                        version.main_class.as_deref().unwrap_or("-")
                    ))
                })
                .collect();
            let list = List::new(items)
                .highlight_style(highlight_style())
                .highlight_symbol("> ");
            app.installed_state
                .select(Some(app.installed_sel.min(app.installed.len() - 1)));
            frame.render_stateful_widget(list, chunks[2], &mut app.installed_state);

            let offset = app.installed_state.offset();
            let mut hits = Vec::new();
            for row in 0..rows {
                let index = offset + row;
                if index >= app.installed.len() {
                    break;
                }
                hits.push((
                    Rect::new(chunks[2].x, chunks[2].y + row as u16, chunks[2].width, 1),
                    RowTarget::Installed,
                    index,
                ));
            }
            app.row_hits.extend(hits);
        }
        VersionTab::Remote => {
            let header = Line::from(Span::styled(
                format!("  {:<22} {:<10} {}", "ID", "TYPE", "RELEASED"),
                Style::default().fg(MUTED),
            ));
            frame.render_widget(Paragraph::new(header), chunks[1]);

            let Some(manifest) = &app.manifest else {
                app.remote_state.select(None);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Remote manifest not loaded. Press l to fetch version_manifest_v2.json.",
                        Style::default().fg(MUTED),
                    ))),
                    chunks[2],
                );
                return;
            };
            let indices = app.remote_indices();
            if indices.is_empty() {
                app.remote_state.select(None);
                let message = if app.remote_filter.is_empty() {
                    "The remote manifest is empty.".to_string()
                } else {
                    format!("No remote versions match '{}'.", app.remote_filter)
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        message,
                        Style::default().fg(MUTED),
                    ))),
                    chunks[2],
                );
                return;
            }
            if rows == 0 {
                return;
            }

            let items: Vec<ListItem> = indices
                .iter()
                .filter_map(|index| manifest.versions.get(*index))
                .map(|version| {
                    let marker = if app.installed.iter().any(|item| item.id == version.id) {
                        "*"
                    } else {
                        " "
                    };
                    let released: String = version.release_time.chars().take(10).collect();
                    ListItem::new(format!(
                        "{marker} {:<22} {:<10} {}",
                        version.id, version.kind, released
                    ))
                })
                .collect();
            let list = List::new(items)
                .highlight_style(highlight_style())
                .highlight_symbol("> ");
            app.remote_state
                .select(Some(app.remote_sel.min(indices.len() - 1)));
            frame.render_stateful_widget(list, chunks[2], &mut app.remote_state);

            let offset = app.remote_state.offset();
            let mut hits = Vec::new();
            for row in 0..rows {
                let index = offset + row;
                if index >= indices.len() {
                    break;
                }
                hits.push((
                    Rect::new(chunks[2].x, chunks[2].y + row as u16, chunks[2].width, 1),
                    RowTarget::Remote,
                    index,
                ));
            }
            app.row_hits.extend(hits);
        }
    }
}

fn draw_accounts(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = match app.current_account_name() {
        Some(name) => format!(" Accounts · current: {name} "),
        None => " Accounts · no account selected ".to_string(),
    };
    let block = page_block(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {:<16} {:<8} {}", "USERNAME", "KIND", "UUID"),
            Style::default().fg(MUTED),
        ))),
        chunks[0],
    );

    let rows = chunks[1].height as usize;
    app.view_rows = rows.max(1);
    if app.accounts.accounts.is_empty() || rows == 0 {
        app.accounts_state.select(None);
        if app.accounts.accounts.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "No accounts. Press a to add an offline account, m to sign in with Microsoft.",
                    Style::default().fg(MUTED),
                ))),
                chunks[1],
            );
        }
        return;
    }

    let selected = app.accounts.selected.clone();
    let items: Vec<ListItem> = app
        .accounts
        .accounts
        .iter()
        .map(|account| {
            let marker = if selected.as_deref() == Some(account.uuid.as_str()) {
                "*"
            } else {
                " "
            };
            ListItem::new(format!(
                "{marker} {:<16} {:<8} {}",
                account.username, account.kind, account.uuid
            ))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(highlight_style())
        .highlight_symbol("> ");
    app.accounts_state
        .select(Some(app.account_sel.min(app.accounts.accounts.len() - 1)));
    frame.render_stateful_widget(list, chunks[1], &mut app.accounts_state);

    let offset = app.accounts_state.offset();
    let mut hits = Vec::new();
    for row in 0..rows {
        let index = offset + row;
        if index >= app.accounts.accounts.len() {
            break;
        }
        hits.push((
            Rect::new(chunks[1].x, chunks[1].y + row as u16, chunks[1].width, 1),
            RowTarget::Account,
            index,
        ));
    }
    app.row_hits.extend(hits);
}

fn draw_settings(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = page_block(" Settings ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let rows = inner.height as usize;
    app.view_rows = rows.max(1);

    let items: Vec<ListItem> = Setting::ALL
        .iter()
        .map(|setting| {
            let suffix = if setting.clearable(app.settings_scope.is_version()) {
                "   [c clear]"
            } else {
                ""
            };
            ListItem::new(format!(
                "{:<20} {}{}",
                setting.label(),
                app.setting_value(*setting),
                suffix
            ))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(highlight_style())
        .highlight_symbol("> ");
    app.settings_state
        .select(Some(app.setting_sel.min(Setting::ALL.len() - 1)));
    frame.render_stateful_widget(list, inner, &mut app.settings_state);

    let offset = app.settings_state.offset();
    let mut hits = Vec::new();
    for row in 0..rows {
        let index = offset + row;
        if index >= Setting::ALL.len() {
            break;
        }
        hits.push((
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            RowTarget::Setting,
            index,
        ));
    }
    app.row_hits.extend(hits);
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.log_offset == 0 {
        " Logs · following ".to_string()
    } else {
        format!(" Logs · {} lines back ", app.log_offset)
    };
    let block = page_block(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.log_area = Some(inner);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let width = inner.width as usize;
    let rows = inner.height as usize;
    app.log_view_rows = rows;
    app.view_rows = rows;

    let mut visual: Vec<String> = Vec::new();
    for line in &app.logs {
        if line.is_empty() {
            visual.push(String::new());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        for chunk in chars.chunks(width) {
            visual.push(chunk.iter().collect());
        }
    }
    app.log_visual_rows = visual.len();

    let max_offset = visual.len().saturating_sub(rows);
    if app.log_offset > max_offset {
        app.log_offset = max_offset;
    }
    let end = visual.len().saturating_sub(app.log_offset);
    let start = end.saturating_sub(rows);

    let lines: Vec<Line> = visual[start..end]
        .iter()
        .map(|line| Line::from(Span::styled(line.clone(), log_style(line))))
        .collect();
    let text = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "No output yet. Install progress, launch commands and game logs appear here.",
            Style::default().fg(MUTED),
        ))]
    } else {
        lines
    };
    frame.render_widget(Paragraph::new(text), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    if chunks[2].height == 0 {
        return;
    }

    let (tag, color) = match app.status.level {
        Level::Info => ("info", INFO),
        Level::Success => ("  ok", ACCENT),
        Level::Error => (" err", ERROR),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(tag, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(app.status.text.clone(), Style::default().fg(Color::Reset)),
        ])),
        chunks[0],
    );

    if let Some(busy) = &app.busy {
        let ratio = if busy.total > 0 {
            (busy.completed as f64 / busy.total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut label = format!(
            "{} · [{}] {}/{} · {}",
            busy.label, busy.phase, busy.completed, busy.total, busy.file
        );
        let width = chunks[1].width as usize;
        if label.chars().count() > width {
            label = label.chars().take(width).collect();
        }
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(ACCENT).bg(Color::Black))
            .ratio(ratio)
            .label(label);
        frame.render_widget(gauge, chunks[1]);
    } else if let Some(version) = &app.running {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("running ", Style::default().fg(ACCENT)),
                Span::styled(
                    format!("Minecraft {version} · output streams to Logs"),
                    Style::default().fg(Color::Reset),
                ),
            ])),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("idle", Style::default().fg(MUTED)))),
            chunks[1],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            help_text(app.page),
            Style::default().fg(MUTED),
        ))),
        chunks[2],
    );
}

fn draw_help(frame: &mut Frame) {
    let area = centered_rect(frame.area(), 78, 24);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(Span::styled(
            " Help ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        help_line("? / F1", "Toggle this help"),
        help_line("1-5", "Switch page (Tab / Shift+Tab cycles)"),
        help_line("r", "Refresh local state and detect Java"),
        help_line("↑ ↓ / j k", "Move selection or scroll logs"),
        help_line("PgUp PgDn", "Jump one screen"),
        help_line("Home End", "Jump to first/last item (logs: oldest/tail)"),
        help_line("Enter", "Run the primary action of the page"),
        help_line("q / Esc", "Quit (Esc closes a dialog first)"),
        Line::from(""),
        help_line("Dashboard", "Enter refreshes and re-detects Java"),
        help_line("Versions", "← → switch tab · i install · l load remote"),
        help_line("", "f filter · d set default · p preview · v verify"),
        help_line(
            "Accounts",
            "a add offline account · m Microsoft sign-in · Enter select",
        ),
        help_line(
            "Settings",
            "Enter edit · c clear · t target · s show config JSON",
        ),
        help_line("Logs", "c clear · mouse wheel scrolls"),
        Line::from(""),
        help_line(
            "Launching",
            "uninstalled versions are installed automatically",
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_modal(frame: &mut Frame, app: &App) {
    let Some(modal) = app.modal.as_ref() else {
        return;
    };
    let area = centered_rect(frame.area(), 76, 9);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(Span::styled(
            format!(" {} ", modal.title),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            modal.prompt.as_str(),
            Style::default().fg(Color::Gray),
        ))),
        chunks[0],
    );

    let input = input_line(modal, chunks[2].width as usize);
    frame.render_widget(Paragraph::new(input.line), chunks[2]);
    frame.set_cursor_position(Position::new(
        chunks[2].x + input.cursor_column as u16,
        chunks[2].y,
    ));

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Enter save · Esc cancel · Ctrl+A select all · ←/→ Home/End move",
            Style::default().fg(MUTED),
        )))
        .wrap(Wrap { trim: false }),
        chunks[3],
    );
}

struct InputLine {
    line: Line<'static>,
    cursor_column: usize,
}

/// Device-code instructions shown while a Microsoft sign-in is pending.
fn draw_microsoft(frame: &mut Frame, app: &App) {
    let Some(code) = app.microsoft.as_ref() else {
        return;
    };
    let area = centered_rect(frame.area(), 74, 11);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(Span::styled(
            " Microsoft sign-in ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let mut lines = vec![
        Line::from(Span::styled(
            "Open this page in a browser:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            code.verification_uri.clone(),
            Style::default().fg(INFO).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Enter the code: ", Style::default().fg(Color::Gray)),
            Span::styled(
                code.user_code.clone(),
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Waiting for Microsoft sign-in...",
        Style::default().fg(ACCENT),
    )));
    lines.push(Line::from(Span::styled(
        "Esc cancels the sign-in",
        Style::default().fg(MUTED),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Build a horizontally scrolled input line keeping the cursor visible.
fn input_line(modal: &InputModal, width: usize) -> InputLine {
    let width = width.max(1);
    let chars: Vec<char> = modal.value.chars().collect();
    let cursor = modal.cursor.min(chars.len());
    let offset = cursor.saturating_sub(width.saturating_sub(1));
    let visible: Vec<char> = chars.iter().skip(offset).take(width).copied().collect();

    let selection = modal.selected_range();
    let mut spans = Vec::new();
    for (index, ch) in visible.iter().enumerate() {
        let absolute = offset + index;
        let selected = selection
            .map(|(start, end)| absolute >= start && absolute < end)
            .unwrap_or(false);
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Gray)
        } else {
            Style::default()
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    if spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    InputLine {
        line: Line::from(spans),
        cursor_column: cursor.saturating_sub(offset),
    }
}

fn field(label: &str, value: String) -> Line<'static> {
    field_styled(label, value, Color::Reset)
}

fn field_styled(label: &str, value: String, value_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<24}"),
            Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, Style::default().fg(value_color)),
    ])
}

fn help_line(keys: &str, description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{keys:<14}"),
            Style::default().fg(INFO).add_modifier(Modifier::BOLD),
        ),
        Span::styled(description.to_string(), Style::default().fg(Color::Gray)),
    ])
}

fn highlight_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn log_style(line: &str) -> Style {
    if line.starts_with("!!!") {
        Style::default().fg(ERROR)
    } else if line.starts_with("==>") {
        Style::default().fg(INFO)
    } else if line.starts_with("[stderr]") {
        Style::default().fg(LABEL)
    } else {
        Style::default().fg(Color::Reset)
    }
}

fn help_text(page: Page) -> &'static str {
    match page {
        Page::Dashboard => "r refresh · Enter re-detect Java · ? help · q quit",
        Page::Versions => {
            "↑/↓ move · Enter launch/install · ←/→ tab · i install · l load remote · f filter · d default · p preview · v verify"
        }
        Page::Accounts => {
            "↑/↓ move · Enter select · a add offline account · m Microsoft sign-in · ? help"
        }
        Page::Settings => {
            "↑/↓ move · Enter edit · c clear · t target · s show config JSON · ? help"
        }
        Page::Logs => "↑/↓ scroll · PgUp/PgDn · Home oldest · End tail · c clear · ? help",
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width).max(1);
    let height = height.min(area.height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
