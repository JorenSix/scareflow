//! Sound file helpers shared by the editor blocks.

use std::path::{Path, PathBuf};

/// Keys that can trigger a Key block. Kept small on purpose: these are the
/// keys a presenter can hit blind.
pub const KEYS: [&str; 9] = ["Space", "Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "1", "2", "3"];

/// Audio files in `root/sub`, as paths relative to `root`, sorted by name.
pub fn list_sounds(root: &Path, sub: &Path) -> Vec<PathBuf> {
    const EXTENSIONS: [&str; 5] = ["wav", "mp3", "ogg", "flac", "m4a"];
    let Ok(entries) = std::fs::read_dir(root.join(sub)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .filter_map(|p| p.file_name().map(|name| sub.join(name)))
        .collect();
    files.sort();
    files
}

/// Short label for a sound path: its file stem.
pub fn label(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The [`KEYS`] entry for a pressed key. egui's own key names differ for
/// some keys (`ArrowUp` is named "Up"), so keys are compared, not names.
pub fn show_key(key: eframe::egui::Key) -> Option<&'static str> {
    KEYS.into_iter().find(|k| eframe::egui::Key::from_name(k) == Some(key))
}

/// Display name for a key from [`KEYS`].
pub fn key_label(key: &str) -> &str {
    match key {
        "ArrowUp" => "Arrow up",
        "ArrowDown" => "Arrow down",
        "ArrowLeft" => "Arrow left",
        "ArrowRight" => "Arrow right",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_show_key_maps_back_to_itself() {
        use eframe::egui::Key;
        for k in KEYS {
            let key = Key::from_name(k).unwrap_or_else(|| panic!("{k} is not an egui key"));
            assert_eq!(show_key(key), Some(k));
        }
        assert_eq!(show_key(Key::ArrowUp), Some("ArrowUp"));
        assert_eq!(show_key(Key::Q), None);
    }

    #[test]
    fn lists_only_audio_files() {
        let root = std::env::temp_dir().join(format!("scareflow-showui-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fx")).unwrap();
        for name in ["b.WAV", "a.mp3", "CREDITS.md"] {
            std::fs::write(root.join("fx").join(name), b"").unwrap();
        }
        assert_eq!(list_sounds(&root, Path::new("fx")), [Path::new("fx/a.mp3"), Path::new("fx/b.WAV")]);
        assert!(list_sounds(&root, Path::new("missing")).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
