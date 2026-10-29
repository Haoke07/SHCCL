mod app;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use sdl3::event::Event;
use sdl3::mouse::MouseButton;
use shccl_core::Launcher;

use crate::app::App;

fn main() -> anyhow::Result<()> {
    let root = parse_root();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let launcher = {
        let _guard = runtime.enter();
        Launcher::new(root)?
    };

    let sdl = sdl3::init()?;
    let video = sdl.video()?;
    let window = video
        .window(
            "SHCCL - Smart Home Control Craft Launcher",
            ui::WINDOW_WIDTH,
            ui::WINDOW_HEIGHT,
        )
        .build()?;
    let mut canvas = window.into_canvas();
    let (output_width, output_height) = canvas.output_size()?;
    canvas.set_scale(
        output_width as f32 / ui::LOGICAL_WIDTH,
        output_height as f32 / ui::LOGICAL_HEIGHT,
    )?;

    // Text input is always enabled; the application only consumes it while an
    // input dialog is open.
    video.text_input().start(canvas.window());

    let mut event_pump = sdl.event_pump()?;
    let mut app = App::new(launcher, runtime.handle().clone());

    while !app.quit {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => app.quit = true,
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    ..
                } => app.handle_key(keycode, keymod),
                Event::TextInput { text, .. } => app.handle_text(&text),
                Event::MouseMotion { x, y, .. } => app.update_mouse(to_logical(x), to_logical(y)),
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => app.handle_click(to_logical(x), to_logical(y)),
                Event::MouseWheel {
                    y,
                    mouse_x,
                    mouse_y,
                    ..
                } => {
                    app.handle_wheel(to_logical(mouse_x), to_logical(mouse_y), y);
                }
                _ => {}
            }
        }

        app.poll_backend();
        app.draw(&mut canvas)?;
        std::thread::sleep(Duration::from_millis(16));
    }

    Ok(())
}

/// Convert window coordinates into the scaled drawing space. The window is not
/// resizable, so the ratio is constant.
fn to_logical(value: f32) -> f32 {
    value * ui::LOGICAL_WIDTH / ui::WINDOW_WIDTH as f32
}

fn parse_root() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--root" {
            return args.next().map(PathBuf::from);
        }
        if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--root=")) {
            return Some(PathBuf::from(value));
        }
        if arg == "--help" || arg == "-h" {
            println!(
                "SHCCL - Smart Home Control Craft Launcher (SDL3 GUI)\n\n\
                 Usage: shccl-gui [--root DIR]\n\n\
                 --root DIR   Launcher root (defaults to SHCCL_ROOT or the platform data directory)"
            );
            std::process::exit(0);
        }
    }
    None
}
