//! Companion tools must sit in the same folder as Podcast Trim.exe.

use crate::store::Settings;
use std::path::{Path, PathBuf};

pub const FILES: [&str; 6] = [
    "ffmpeg.exe",
    "ffprobe.exe",
    "whisper-cli.exe",
    "whisper.dll",
    "ggml.dll",
    "ggml-base.dll",
];

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|n| n.to_ascii_lowercase());
    v
}

/// Whisper models (`ggml-*.bin`) in `dir`, sorted by name.
pub fn models(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.starts_with("ggml-") && l.ends_with(".bin")
        })
        .collect()
}

/// What's missing from `dir`, in plain words. Empty when everything is there.
pub fn missing(dir: &Path) -> Vec<String> {
    let have: Vec<String> = names(dir).iter().map(|n| n.to_ascii_lowercase()).collect();
    let mut out: Vec<String> = FILES
        .iter()
        .filter(|f| !have.contains(&f.to_string()))
        .map(|f| f.to_string())
        .collect();
    if !have
        .iter()
        .any(|n| n.starts_with("ggml-cpu") && n.ends_with(".dll"))
    {
        out.push("ggml-cpu-*.dll (from the whisper.cpp zip)".into());
    }
    if models(dir).is_empty() {
        out.push("a Whisper model such as ggml-base.en.bin".into());
    }
    out
}

pub fn missing_message(dir: &Path, missing: &[String]) -> String {
    format!(
        "Missing from {}: {}. Put ffmpeg.exe and ffprobe.exe (FFmpeg), whisper-cli.exe with its DLLs (whisper.cpp), and a ggml-*.bin model in the same folder as Podcast Trim.exe.",
        dir.display(),
        missing.join(", ")
    )
}

/// Settings with tool paths filled in from `dir`. The chosen model is used
/// if present, otherwise the first one found.
pub fn resolve(settings: &Settings, dir: &Path) -> Result<Settings, String> {
    let miss = missing(dir);
    if !miss.is_empty() {
        return Err(missing_message(dir, &miss));
    }
    let models = models(dir);
    let model = models
        .iter()
        .find(|m| m.eq_ignore_ascii_case(&settings.whisper_model))
        .unwrap_or(&models[0]);
    let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
    Ok(Settings {
        ffmpeg: p("ffmpeg.exe"),
        ffprobe: p("ffprobe.exe"),
        whisper_cli: p("whisper-cli.exe"),
        whisper_model: p(model),
        ..settings.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_and_reports() {
        let d = std::env::temp_dir().join(format!("pt-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let m = missing(&d);
        assert_eq!(m.len(), 8);
        assert!(resolve(&Settings::default(), &d)
            .unwrap_err()
            .contains("ffmpeg.exe"));
        for f in FILES.iter().chain(
            [
                "ggml-cpu-x64.dll",
                "ggml-small.en.bin",
                "ggml-base.en.bin",
                "for-tests-ggml-tiny.bin",
            ]
            .iter(),
        ) {
            fs::write(d.join(f), b"x").unwrap();
        }
        assert!(missing(&d).is_empty());
        assert_eq!(models(&d), ["ggml-base.en.bin", "ggml-small.en.bin"]);
        let s = Settings {
            whisper_model: "ggml-small.en.bin".into(),
            ..Default::default()
        };
        let r = resolve(&s, &d).unwrap();
        assert!(r.whisper_model.ends_with("ggml-small.en.bin") && r.ffmpeg.ends_with("ffmpeg.exe"));
        let r = resolve(
            &Settings {
                whisper_model: "gone.bin".into(),
                ..Default::default()
            },
            &d,
        )
        .unwrap();
        assert!(r.whisper_model.ends_with("ggml-base.en.bin"));
        fs::remove_dir_all(d).unwrap();
    }
}
