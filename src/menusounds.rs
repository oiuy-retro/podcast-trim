//! Optional menu sounds, loaded at runtime from the user's own Half-Life
//! install. Nothing from the game is shipped with this program.
//!
//! configuration.conf:
//! ```text
//! [secret]
//! menu-sounds = on
//! menu-sounds-dir = auto
//! ```

use crate::config::{expand_vars, Conf};
use std::path::{Path, PathBuf};

/// GameUI button sounds (`sound/UI/` in the game): hover, press, release.
pub const ROLLOVER: &str = "buttonrollover.wav";
pub const CLICK: &str = "buttonclick.wav";
pub const RELEASE: &str = "buttonclickrelease.wav";
pub const FILES: [&str; 3] = [ROLLOVER, CLICK, RELEASE];

pub fn enabled(conf: &Conf) -> bool {
    matches!(
        conf.get("secret", "menu-sounds")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "on" | "yes" | "true" | "1"
    )
}

/// Library folders listed in Steam's `libraryfolders.vdf` (`"path" "D:\\Lib"`).
pub fn vdf_libraries(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split('"').collect();
            // ["\t\t", "path", "\t\t", "D:\\\\Lib", ""]
            (parts.len() >= 5 && parts[1].eq_ignore_ascii_case("path"))
                .then(|| PathBuf::from(parts[3].replace("\\\\", "\\")))
        })
        .collect()
}

fn ui_dir(library: &Path) -> PathBuf {
    library
        .join("steamapps")
        .join("common")
        .join("Half-Life")
        .join("valve")
        .join("sound")
        .join("ui")
}

/// Folders `auto` tries, in order.
pub fn auto_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![exe_dir.join("sounds")];
    let mut steams = Vec::new();
    for var in ["%ProgramFiles(x86)%\\Steam", "%ProgramFiles%\\Steam"] {
        let p = expand_vars(var);
        if !p.contains('%') {
            steams.push(PathBuf::from(p));
        }
    }
    for steam in steams {
        out.push(ui_dir(&steam));
        let vdf = steam.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(vdf) {
            for lib in vdf_libraries(&text) {
                let d = ui_dir(&lib);
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
    }
    out
}

fn has_sounds(dir: &Path) -> bool {
    FILES.iter().any(|f| dir.join(f).is_file())
}

/// The folder holding the sounds, or None (stay quiet).
pub fn locate(conf: &Conf, exe_dir: &Path) -> Option<PathBuf> {
    if !enabled(conf) {
        return None;
    }
    let value = conf.get("secret", "menu-sounds-dir").unwrap_or_default();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return auto_candidates(exe_dir).into_iter().find(|d| has_sounds(d));
    }
    let mut p = PathBuf::from(expand_vars(&value));
    if p.is_relative() {
        p = exe_dir.join(p);
    }
    has_sounds(&p).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pt-ms-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn off_by_default() {
        assert!(!enabled(&Conf::parse("")));
        assert!(!enabled(&Conf::parse(crate::config::TEMPLATE)));
        assert!(!crate::config::TEMPLATE.contains("secret"));
        assert!(enabled(&Conf::parse("[secret]\nmenu-sounds = On\n")));
        assert!(!enabled(&Conf::parse("[secret]\nmenu-sounds = off\n")));
        assert!(!enabled(&Conf::parse("[tools]\nmenu-sounds = on\n")));
    }

    #[test]
    fn vdf() {
        let v = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            vdf_libraries(v),
            vec![
                PathBuf::from("C:\\Program Files (x86)\\Steam"),
                PathBuf::from("D:\\SteamLibrary")
            ]
        );
    }

    #[test]
    fn locate_dirs() {
        let exe = tmp("exe");
        let on = "[secret]\nmenu-sounds = on\n";
        assert_eq!(locate(&Conf::parse(on), &exe), locate(&Conf::parse(on), &exe));
        std::fs::create_dir_all(exe.join("sounds")).unwrap();
        std::fs::write(exe.join("sounds").join(CLICK), b"RIFF").unwrap();
        assert_eq!(locate(&Conf::parse(on), &exe), Some(exe.join("sounds")));
        assert_eq!(locate(&Conf::parse("[secret]\nmenu-sounds = off\n"), &exe), None);
        let other = tmp("other");
        std::fs::write(other.join(ROLLOVER), b"RIFF").unwrap();
        let c = format!("{on}menu-sounds-dir = {}\n", other.display());
        assert_eq!(locate(&Conf::parse(&c), &exe), Some(other.clone()));
        let rel = format!("{on}menu-sounds-dir = sounds\n");
        assert_eq!(locate(&Conf::parse(&rel), &exe), Some(exe.join("sounds")));
        let missing = format!("{on}menu-sounds-dir = {}\n", exe.join("nope").display());
        assert_eq!(locate(&Conf::parse(&missing), &exe), None);
        let _ = std::fs::remove_dir_all(exe);
        let _ = std::fs::remove_dir_all(other);
    }
}
