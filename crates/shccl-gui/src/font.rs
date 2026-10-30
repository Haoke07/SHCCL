//! Picks a monospace font from the fonts installed on the system and scales it
//! to the height the UI layout expects.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use sdl3::ttf::{self, Font};

/// Height a single line of UI text should occupy, in logical (640x400) units.
/// The layout is built on a 14px row grid with 12px sub-lines, so the font is
/// scaled to match and then centered inside those rows.
const TEXT_HEIGHT: f32 = 12.0;

/// A font file worth trying. Anything with a monospace-sounding name is tried
/// before the remaining fonts found on the system.
const MONOSPACE_HINTS: [&str; 10] = [
    "mono", "consol", "courier", "menlo", "monaco", "fixed", "code", "hack", "iosevka", "cascadia",
];

const FONT_EXTENSIONS: [&str; 4] = ["ttf", "ttc", "otf", "otc"];

/// A system monospace font scaled for the UI, together with the metrics the
/// drawing code needs to lay out and clip text.
///
/// The font is rasterized at the device resolution so glyphs are never scaled
/// up by the renderer, while the metrics stay in logical (640x400) units so the
/// layout code can keep working in its own coordinate space.
pub(crate) struct UiFont {
    font: Font<'static>,
    scale: f32,
    char_width: f32,
    line_height: f32,
}

impl UiFont {
    /// Loads the first usable monospace font installed on the system.
    ///
    /// `scale` is the number of device pixels per logical unit.
    pub(crate) fn load_system(context: &ttf::Sdl3TtfContext, scale: f32) -> Result<Self> {
        let scale = scale.max(0.1);
        for path in candidates() {
            if let Some(font) = load_monospace(context, &path, scale) {
                return Ok(font);
            }
        }
        bail!(
            "no system monospace font found; install one (for example DejaVu Sans Mono) \
             to run the GUI"
        )
    }

    pub(crate) fn font(&self) -> &Font<'static> {
        &self.font
    }

    /// Device pixels per logical unit, used to place shapes and text.
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Advance width of a single character, used for clipping and centering.
    pub(crate) fn char_width(&self) -> f32 {
        self.char_width
    }

    /// Height of one rendered line, used to center text inside rows.
    pub(crate) fn line_height(&self) -> f32 {
        self.line_height
    }
}

/// Loads `path` and returns it scaled for the UI, or `None` when the file is
/// not a font or does not contain a monospace face.
fn load_monospace(context: &ttf::Sdl3TtfContext, path: &Path, scale: f32) -> Option<UiFont> {
    let target_height = TEXT_HEIGHT * scale;
    let font = context.load_font(path, target_height).ok()?;
    if !font.face_is_fixed_width() {
        return None;
    }

    // Binary search for the largest size whose line still fits in the row grid
    // so rows and sub-lines keep the spacing the layout was built around.
    let mut low = target_height * 0.25;
    let mut high = target_height * 4.0;
    for _ in 0..10 {
        let mid = (low + high) / 2.0;
        if font.set_size(mid).is_err() {
            break;
        }
        if (font.height() as f32) <= target_height {
            low = mid;
        } else {
            high = mid;
        }
    }
    font.set_size(low.max(1.0)).ok()?;

    let line_height = font.height() as f32 / scale;
    let char_width = font
        .size_of("M")
        .ok()
        .map(|(width, _)| width as f32 / scale)
        .filter(|width| *width > 0.0)?;
    Some(UiFont {
        font,
        scale,
        char_width,
        line_height,
    })
}

/// Font files that may contain a monospace face, best candidates first.
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // fontconfig knows which monospace face the user prefers on most Unix
    // systems, so ask it before scanning directories ourselves.
    if let Some(path) = fontconfig_monospace() {
        paths.push(path);
    }
    paths.extend(known_paths().into_iter().filter(|path| path.is_file()));

    let mut named = Vec::new();
    let mut rest = Vec::new();
    for directory in font_directories() {
        collect_fonts(&directory, &mut named, &mut rest);
    }
    paths.extend(named);
    paths.extend(rest);

    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

/// Asks fontconfig for the file backing the user's preferred monospace font.
fn fontconfig_monospace() -> Option<PathBuf> {
    let output = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// Well-known system font locations, checked before scanning directories.
fn known_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(windir) = env::var_os("WINDIR") {
            let fonts = PathBuf::from(windir).join("Fonts");
            for name in ["consola.ttf", "lucon.ttf", "cour.ttf", "CascadiaMono.ttf"] {
                paths.push(fonts.join(name));
            }
        }
    } else if cfg!(target_os = "macos") {
        for path in [
            "/System/Library/Fonts/SFNSMono.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Monaco.ttf",
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
        ] {
            paths.push(PathBuf::from(path));
        }
    } else {
        for path in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansMono-Regular.ttf",
        ] {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// Directories that hold system and per-user fonts on this platform.
fn font_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(windir) = env::var_os("WINDIR") {
            directories.push(PathBuf::from(windir).join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        for path in [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ] {
            directories.push(PathBuf::from(path));
        }
    } else {
        directories.push(PathBuf::from("/usr/share/fonts"));
        directories.push(PathBuf::from("/usr/local/share/fonts"));
    }

    if let Some(home) = home_directory() {
        for path in [".local/share/fonts", ".fonts", "Library/Fonts"] {
            directories.push(home.join(path));
        }
    }
    if cfg!(target_os = "windows")
        && let Some(local) = env::var_os("LOCALAPPDATA")
    {
        directories.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    directories
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn collect_fonts(directory: &Path, named: &mut Vec<PathBuf>, rest: &mut Vec<PathBuf>) {
    collect_fonts_at(directory, named, rest, 0);
}

fn collect_fonts_at(
    directory: &Path,
    named: &mut Vec<PathBuf>,
    rest: &mut Vec<PathBuf>,
    depth: usize,
) {
    // Bounded so a huge font tree cannot stall startup.
    if depth > 3 || rest.len() > 256 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fonts_at(&path, named, rest, depth + 1);
        } else if is_font_file(&path) {
            if looks_monospace(&path) {
                named.push(path);
            } else {
                rest.push(path);
            }
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            FONT_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

fn looks_monospace(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    MONOSPACE_HINTS.iter().any(|hint| name.contains(hint))
}
