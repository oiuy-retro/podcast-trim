//! Opt-in end-to-end run with the real companion tools. Skipped unless these
//! are set: PT_FFMPEG, PT_FFPROBE, PT_WHISPER, PT_MODEL, PT_SAMPLE (a short
//! speech clip, e.g. whisper.cpp's samples/jfk.wav).

use podcast_trim::pipeline::{self, Env};
use podcast_trim::store::{Episode, Paths, Settings, State};
use std::path::Path;

struct Real {
    sample: String,
}

impl Env for Real {
    fn fetch(&self, _url: &str, dest: &Path) -> Result<(), String> {
        std::fs::copy(&self.sample, dest)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        let o = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }
}

#[test]
fn real_tools() {
    let var = |k: &str| std::env::var(k).ok();
    let (Some(ff), Some(fp), Some(wh), Some(model), Some(sample)) = (
        var("PT_FFMPEG"),
        var("PT_FFPROBE"),
        var("PT_WHISPER"),
        var("PT_MODEL"),
        var("PT_SAMPLE"),
    ) else {
        eprintln!("skipped: PT_* not set");
        return;
    };
    let root = std::env::temp_dir().join(format!("pt-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths::new(root.clone());
    let s = Settings {
        ffmpeg: ff,
        ffprobe: fp,
        whisper_cli: wh,
        whisper_model: model,
        threads: 4,
        ..Default::default()
    };
    let env = Real { sample };
    let mut ep = Episode {
        guid: "e2e".into(),
        title: "E2E".into(),
        pub_date: String::new(),
        source_url: "local".into(),
        state: State::New,
        record: Default::default(),
        cleaned_bytes: 0,
        served: 0,
    };
    pipeline::download(&env, &s, &paths, 1, &mut ep).unwrap();
    pipeline::transcribe(&env, &s, &paths, 1, &mut ep).unwrap();
    println!("duration {}\n{}", ep.record.duration, ep.record.transcript);
    let cuts = pipeline::check_reply(&ep, "00:00:02.000-00:00:04.000").unwrap();
    pipeline::apply_cuts(&env, &s, &paths, 1, &mut ep, cuts).unwrap();
    let out = paths.cleaned(1, &ep);
    let d: f64 = env
        .run(&s.ffprobe, &podcast_trim::ffargs::probe_args(&out))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    println!("cleaned duration {d}");
    assert!((d - (ep.record.duration - 2.0)).abs() < 0.2);
    pipeline::mark_delivered(&paths, 1, &mut ep);
    pipeline::regenerate(&env, &s, &paths, 1, &mut ep).unwrap();
    assert!(paths.cleaned(1, &ep).exists());
}
