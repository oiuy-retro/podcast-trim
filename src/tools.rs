//! Finds ffmpeg, ffprobe, whisper-cli and the Whisper models the way
//! `configuration.conf` says (see config.rs).

use crate::config::{self, find_tool, Conf};
use crate::store::Settings;
use std::path::{Path, PathBuf};

/// DLLs whisper-cli.exe loads from its own folder.
pub const WHISPER_DLLS: [&str; 3] = ["whisper.dll", "ggml.dll", "ggml-base.dll"];

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn files(dir: &Path) -> Vec<String> {
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

fn is_model(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.starts_with("ggml-") && l.ends_with(".bin")
}

/// Whisper models as (name shown in Options, full path). With
/// `whisper-model = auto` that is every `ggml-*.bin` in the auto folders,
/// named relative to the exe's folder; otherwise just the configured file.
pub fn models(conf: &Conf, dir: &Path) -> Vec<(String, PathBuf)> {
    let value = conf.get("tools", "whisper-model").unwrap_or_default();
    let v = value.trim();
    if !(v.is_empty() || v.eq_ignore_ascii_case("auto")) {
        let p = dir.join(config::expand_vars(v));
        return if p.is_file() {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            vec![(name, p)]
        } else {
            vec![]
        };
    }
    let mut out = vec![];
    for d in config::auto_dirs(dir) {
        for f in files(&d).into_iter().filter(|f| is_model(f)) {
            let p = d.join(&f);
            let name = p
                .strip_prefix(dir)
                .map(|r| r.to_string_lossy().replace('/', "\\"))
                .unwrap_or(f);
            out.push((name, p));
        }
    }
    out
}

pub fn model_names(conf: &Conf, dir: &Path) -> Vec<String> {
    models(conf, dir).into_iter().map(|(n, _)| n).collect()
}

struct Found {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    whisper_cli: PathBuf,
    models: Vec<(String, PathBuf)>,
}

fn locate(conf: &Conf, dir: &Path) -> (Option<Found>, Vec<String>) {
    let mut problems = vec![];
    let mut get = |key: &str, file: &str| {
        find_tool(conf, key, file, dir)
            .map_err(|e| problems.push(e))
            .ok()
    };
    let ffmpeg = get("ffmpeg", "ffmpeg.exe");
    let ffprobe = get("ffprobe", "ffprobe.exe");
    let whisper_cli = get("whisper-cli", "whisper-cli.exe");
    if let Some(w) = &whisper_cli {
        let wdir = w.parent().unwrap_or(dir);
        let have: Vec<String> = files(wdir).iter().map(|n| n.to_ascii_lowercase()).collect();
        let mut miss: Vec<String> = WHISPER_DLLS
            .iter()
            .filter(|f| !have.contains(&f.to_string()))
            .map(|f| f.to_string())
            .collect();
        if !have.iter().any(|n| n.starts_with("ggml-cpu") && n.ends_with(".dll")) {
            miss.push("ggml-cpu-*.dll".into());
        }
        if !miss.is_empty() {
            problems.push(format!(
                "{} must be next to whisper-cli.exe in {} (they come in the whisper.cpp zip)",
                miss.join(", "),
                wdir.display()
            ));
        }
    }
    let models = models(conf, dir);
    if models.is_empty() {
        let (v, line) = conf
            .get_line("tools", "whisper-model")
            .unwrap_or(("auto".into(), 0));
        problems.push(if v.trim().is_empty() || v.trim().eq_ignore_ascii_case("auto") {
            "no Whisper model (ggml-*.bin, such as ggml-base.en.bin) next to Podcast Trim.exe or in a folder inside it".into()
        } else {
            format!(
                "{} doesn't exist ({} line {line}, whisper-model = {v})",
                dir.join(config::expand_vars(v.trim())).display(),
                config::FILE_NAME
            )
        });
    }
    let found = match (ffmpeg, ffprobe, whisper_cli) {
        (Some(ffmpeg), Some(ffprobe), Some(whisper_cli)) if problems.is_empty() => Some(Found {
            ffmpeg,
            ffprobe,
            whisper_cli,
            models,
        }),
        _ => None,
    };
    (found, problems)
}

/// What can't be found, in plain words. Empty when everything is there.
pub fn missing(conf: &Conf, dir: &Path) -> Vec<String> {
    locate(conf, dir).1
}

pub fn missing_message(conf: &Conf, missing: &[String]) -> String {
    let file = conf
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| config::FILE_NAME.into());
    format!(
        "Can't find: {}. Put the programs next to Podcast Trim.exe or in a folder inside it, or set where they are in {file}.",
        missing.join("; ")
    )
}

/// Settings with tool paths filled in. The model picked in Options is used
/// if it's still there, otherwise the first one found.
pub fn resolve(settings: &Settings, conf: &Conf, dir: &Path) -> Result<Settings, String> {
    let (found, problems) = locate(conf, dir);
    let Some(f) = found else {
        return Err(missing_message(conf, &problems));
    };
    let pick = |m: &&(String, PathBuf)| {
        m.0.eq_ignore_ascii_case(&settings.whisper_model)
            || m.1
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&settings.whisper_model))
    };
    let model = f.models.iter().find(pick).unwrap_or(&f.models[0]);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    Ok(Settings {
        ffmpeg: s(&f.ffmpeg),
        ffprobe: s(&f.ffprobe),
        whisper_cli: s(&f.whisper_cli),
        whisper_model: s(&model.1),
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
        fs::create_dir_all(d.join("ffmpeg-9.0/bin")).unwrap();
        fs::create_dir_all(d.join("whisper")).unwrap();
        fs::create_dir_all(d.join("models")).unwrap();
        let conf = Conf::parse(config::TEMPLATE);
        let m = missing(&conf, &d);
        assert_eq!(m.len(), 4, "{m:?}");
        assert!(resolve(&Settings::default(), &conf, &d)
            .unwrap_err()
            .contains("ffmpeg.exe"));
        for f in ["ffmpeg.exe", "ffprobe.exe"] {
            fs::write(d.join("ffmpeg-9.0/bin").join(f), b"x").unwrap();
        }
        fs::write(d.join("whisper/whisper-cli.exe"), b"x").unwrap();
        let m = missing(&conf, &d);
        assert!(m[0].contains("ggml-cpu-*.dll") && m[0].contains("whisper"), "{m:?}");
        for f in WHISPER_DLLS.iter().chain(&["ggml-cpu-x64.dll"]) {
            fs::write(d.join("whisper").join(f), b"x").unwrap();
        }
        fs::write(d.join("ggml-base.en.bin"), b"x").unwrap();
        fs::write(d.join("models/ggml-small.en.bin"), b"x").unwrap();
        fs::write(d.join("models/notes.txt"), b"x").unwrap();
        assert!(missing(&conf, &d).is_empty());
        assert_eq!(
            model_names(&conf, &d),
            ["ggml-base.en.bin", "models\\ggml-small.en.bin"]
        );
        let s = Settings {
            whisper_model: "models\\ggml-small.en.bin".into(),
            ..Default::default()
        };
        let r = resolve(&s, &conf, &d).unwrap();
        assert!(r.whisper_model.ends_with("ggml-small.en.bin"));
        assert!(r.ffmpeg.ends_with("ffmpeg.exe") && r.ffmpeg.contains("bin"));
        let s = Settings {
            whisper_model: "gone.bin".into(),
            ..Default::default()
        };
        assert!(resolve(&s, &conf, &d).unwrap().whisper_model.ends_with("ggml-base.en.bin"));
        // A fixed model path in the file wins over the list.
        let mut c2 = conf.clone();
        c2.set("tools", "whisper-model", "models/ggml-small.en.bin");
        assert_eq!(model_names(&c2, &d), ["ggml-small.en.bin"]);
        c2.set("tools", "whisper-model", "nope.bin");
        assert!(missing(&c2, &d)[0].contains("whisper-model = nope.bin"));
        fs::remove_dir_all(d).unwrap();
    }
}
