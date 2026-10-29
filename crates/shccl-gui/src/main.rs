use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use sdl3::event::Event;
use sdl3::keyboard::Keycode;
use sdl3::pixels::Color;
use sdl3::render::{FPoint, FRect};
use shccl_core::{AccountStore, InstalledVersionSummary, Launcher};

const WINDOW_WIDTH: u32 = 1024;
const WINDOW_HEIGHT: u32 = 640;
const SCALE: f32 = 2.0;
const WIDTH: f32 = WINDOW_WIDTH as f32 / SCALE;
const HEIGHT: f32 = WINDOW_HEIGHT as f32 / SCALE;

const BACKGROUND: Color = Color::RGB(14, 17, 22);
const PANEL: Color = Color::RGB(24, 30, 38);
const ACCENT: Color = Color::RGB(86, 214, 134);
const TEXT: Color = Color::RGB(226, 232, 240);
const MUTED: Color = Color::RGB(130, 143, 160);

struct State {
    launcher: Launcher,
    installed: Vec<InstalledVersionSummary>,
    accounts: AccountStore,
    status: String,
}

impl State {
    fn new(launcher: Launcher) -> Self {
        let mut state = Self {
            launcher,
            installed: Vec::new(),
            accounts: AccountStore::default(),
            status: String::new(),
        };
        state.refresh();
        state
    }

    fn refresh(&mut self) {
        self.accounts = self.launcher.accounts();
        match self.launcher.installed_versions() {
            Ok(installed) => {
                self.installed = installed;
                self.status = format!(
                    "{} installed versions | {} accounts",
                    self.installed.len(),
                    self.accounts.accounts.len()
                );
            }
            Err(error) => {
                self.installed.clear();
                self.status = format!("error: {error}");
            }
        }
    }

    fn draw(&self, canvas: &mut sdl3::render::Canvas<sdl3::video::Window>) -> Result<()> {
        canvas.set_draw_color(BACKGROUND);
        canvas.clear();

        canvas.set_draw_color(PANEL);
        canvas.fill_rect(FRect::new(12.0, 12.0, WIDTH - 24.0, 56.0))?;
        canvas.fill_rect(FRect::new(12.0, 76.0, 210.0, HEIGHT - 100.0))?;
        canvas.fill_rect(FRect::new(230.0, 76.0, WIDTH - 242.0, HEIGHT - 100.0))?;

        canvas.set_draw_color(ACCENT);
        canvas.fill_rect(FRect::new(12.0, 12.0, 5.0, 56.0))?;

        canvas.set_draw_color(TEXT);
        canvas.draw_debug_text("SHCCL", FPoint::new(26.0, 20.0))?;
        canvas.draw_debug_text("Smart Home Control Craft Launcher", FPoint::new(26.0, 34.0))?;
        canvas.set_draw_color(MUTED);
        canvas.draw_debug_text(
            "SDL3 frontend scaffold (CLI-first build)",
            FPoint::new(26.0, 48.0),
        )?;

        canvas.set_draw_color(ACCENT);
        canvas.draw_debug_text("STATUS", FPoint::new(24.0, 88.0))?;
        canvas.set_draw_color(TEXT);
        let root = self.launcher.paths().root().display().to_string();
        canvas.draw_debug_text(&format!("root: {root}"), FPoint::new(24.0, 108.0))?;
        canvas.draw_debug_text(
            &format!("memory: {} MiB", self.launcher.config().memory_mb),
            FPoint::new(24.0, 122.0),
        )?;
        let account = self
            .accounts
            .current()
            .map(|account| account.username.clone())
            .unwrap_or_else(|| "none".to_string());
        canvas.draw_debug_text(&format!("account: {account}"), FPoint::new(24.0, 136.0))?;
        canvas.draw_debug_text(
            &format!("installed: {}", self.installed.len()),
            FPoint::new(24.0, 150.0),
        )?;
        canvas.draw_debug_text("Use the CLI:", FPoint::new(24.0, 180.0))?;
        canvas.set_draw_color(MUTED);
        canvas.draw_debug_text("shccl login offline <name>", FPoint::new(24.0, 198.0))?;
        canvas.draw_debug_text("shccl install <version>", FPoint::new(24.0, 212.0))?;
        canvas.draw_debug_text("shccl launch <version>", FPoint::new(24.0, 226.0))?;

        canvas.set_draw_color(ACCENT);
        canvas.draw_debug_text("INSTALLED VERSIONS", FPoint::new(242.0, 88.0))?;
        canvas.set_draw_color(TEXT);
        if self.installed.is_empty() {
            canvas.set_draw_color(MUTED);
            canvas.draw_debug_text("No installed versions yet.", FPoint::new(242.0, 108.0))?;
        } else {
            for (index, version) in self.installed.iter().take(15).enumerate() {
                canvas.draw_debug_text(
                    &format!("{:<20} {}", version.id, version.version_type),
                    FPoint::new(242.0, 108.0 + index as f32 * 14.0),
                )?;
            }
        }

        canvas.set_draw_color(MUTED);
        canvas.draw_debug_text(&self.status, FPoint::new(24.0, HEIGHT - 16.0))?;
        canvas.draw_debug_text(
            "R refresh   Q / Esc quit",
            FPoint::new(WIDTH - 154.0, HEIGHT - 16.0),
        )?;

        canvas.present();
        Ok(())
    }
}

fn main() -> Result<()> {
    let launcher = Launcher::new(parse_root())?;
    let mut state = State::new(launcher);

    let sdl = sdl3::init()?;
    let video = sdl.video()?;
    let window = video
        .window(
            "SHCCL - Smart Home Control Craft Launcher",
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        )
        .build()?;
    let mut canvas = window.into_canvas();
    canvas.set_scale(SCALE, SCALE)?;
    let mut event_pump = sdl.event_pump()?;

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(Keycode::Escape | Keycode::Q),
                    ..
                } => break 'running,
                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    ..
                } => state.refresh(),
                _ => {}
            }
        }

        state.draw(&mut canvas)?;
        std::thread::sleep(Duration::from_millis(16));
    }

    Ok(())
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
