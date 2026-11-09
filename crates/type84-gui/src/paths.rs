//! Where the GUI keeps its files. Every path is injectable so tests run
//! against a temporary directory and never touch the user's real state.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File locations used by the GUI and its HID worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `type84` state dir: the write lock (`anim.lock`) and the lighting
    /// last-write record (`lighting-last.json`), shared with the CLI.
    pub state_dir: PathBuf,
    /// Automatic pre-write backups.
    pub auto_backup_dir: PathBuf,
    /// Backups made with «Сделать бэкап».
    pub backup_dir: PathBuf,
    /// Presets, shared with `type84 preset`.
    pub presets_dir: PathBuf,
    /// GUI settings (`gui.json`).
    pub settings_file: PathBuf,
}

impl Paths {
    /// The user's real locations: the CLI's state dir
    /// (`~/.local/state/type84`), backups under `<state dir>/backups`, and
    /// the CLI's preset dir (`~/.config/type84/presets`).
    pub fn user() -> anyhow::Result<Self> {
        let state_dir = type84_cli::anim::state_dir()?;
        let presets_dir = type84_cli::preset::default_dir()?;
        let settings_file = presets_dir
            .parent()
            .map(|d| d.join("gui.json"))
            .unwrap_or_else(|| state_dir.join("gui.json"));
        Ok(Self::with(state_dir, presets_dir, settings_file))
    }

    /// Everything under one directory (tests).
    pub fn under(root: &Path) -> Self {
        Self::with(
            root.join("state"),
            root.join("presets"),
            root.join("gui.json"),
        )
    }

    fn with(state_dir: PathBuf, presets_dir: PathBuf, settings_file: PathBuf) -> Self {
        Paths {
            auto_backup_dir: state_dir.join("backups").join("auto"),
            backup_dir: state_dir.join("backups"),
            state_dir,
            presets_dir,
            settings_file,
        }
    }
}

/// GUI preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiSettings {
    /// Closing the window hides it to the tray instead of quitting.
    #[serde(default = "yes")]
    pub close_to_tray: bool,
}

fn yes() -> bool {
    true
}

impl Default for GuiSettings {
    fn default() -> Self {
        GuiSettings {
            close_to_tray: true,
        }
    }
}

impl GuiSettings {
    /// Load, falling back to the defaults for a missing or broken file.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Newest `*.json` file in `dir` by modification time.
pub fn newest_json(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// An absolute version of `p` for messages (falls back to `p`).
pub fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("type84-gui-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Paths::under(&dir);
        assert!(GuiSettings::load(&p.settings_file).close_to_tray);
        let s = GuiSettings {
            close_to_tray: false,
        };
        s.save(&p.settings_file).unwrap();
        assert_eq!(GuiSettings::load(&p.settings_file), s);
        std::fs::write(&p.settings_file, "{").unwrap();
        assert!(GuiSettings::load(&p.settings_file).close_to_tray);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
