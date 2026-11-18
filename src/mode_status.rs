use crate::window::DesktopMode;
use std::{env, fs, path::PathBuf};

const STATUS_FILE: &str = "anwilite-octlitch-0-mode.json";

pub struct ModeStatus {
    path: Option<PathBuf>,
    last: Option<(usize, DesktopMode)>,
    warned: bool,
}

impl ModeStatus {
    pub fn new() -> Self {
        let path = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(STATUS_FILE));
        Self {
            path,
            last: None,
            warned: false,
        }
    }

    /// Write a single JSON line atomically; skip writes when unchanged.
    pub fn update(&mut self, workspace: usize, mode: DesktopMode) {
        if self.last == Some((workspace, mode)) {
            return;
        }
        let Some(path) = self.path.as_ref() else {
            if !self.warned {
                eprintln!(
                    "Anwilite mode indicator: XDG_RUNTIME_DIR missing; Waybar status disabled"
                );
                self.warned = true;
            }
            return;
        };
        let temporary = path.with_extension("tmp");
        let status = mode_json(workspace, mode);
        if let Err(error) = fs::write(&temporary, status).and_then(|_| fs::rename(&temporary, path))
        {
            if !self.warned {
                eprintln!(
                    "Anwilite mode indicator: cannot write {}: {error}",
                    path.display()
                );
                self.warned = true;
            }
            return;
        }
        self.last = Some((workspace, mode));
        self.warned = false;
    }
}

impl Drop for ModeStatus {
    fn drop(&mut self) {
        if self.last.is_some() {
            if let Some(path) = &self.path {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn mode_json(workspace: usize, mode: DesktopMode) -> String {
    let (label, class) = match mode {
        DesktopMode::Floating => ("Windowed", "windowed"),
        DesktopMode::Tiled => ("Tiled", "tiled"),
        DesktopMode::Grid => ("Grid", "grid"),
    };
    format!(
        r#"{{"text":"WS {workspace} | {label}","tooltip":"Anwilite — workspace {workspace} — {label}","class":"{class}"}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_valid_waybar_fields_for_all_modes() {
        for (mode, label, class) in [
            (DesktopMode::Floating, "Windowed", "windowed"),
            (DesktopMode::Tiled, "Tiled", "tiled"),
            (DesktopMode::Grid, "Grid", "grid"),
        ] {
            let text = mode_json(3, mode);
            assert!(text.contains(&format!(r#""text":"WS 3 | {label}""#)));
            assert!(text.contains(&format!(r#""class":"{class}""#)));
            assert!(text.contains("workspace 3"));
            assert_eq!(text.lines().count(), 1);
        }
    }
}
