//! Opt-in end-to-end run with the real companion tools. Skipped unless
//! PT_TOOLS_DIR (a folder laid out like the app folder: ffmpeg.exe,
//! ffprobe.exe, whisper-cli.exe and its DLLs, a ggml-*.bin model) and
//! PT_SAMPLE (a short speech clip, e.g. whisper.cpp's samples/jfk.mp3) are set.

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
    let (Ok(dir), Ok(sample)) = (std::env::var("PT_TOOLS_DIR"), std::env::var("PT_SAMPLE")) else {
        eprintln!("skipped: PT_TOOLS_DIR / PT_SAMPLE not set");
        return;
    };
    let root = std::env::temp_dir().join(format!("pt-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths::new(root.clone());
    let s = podcast_trim::tools::resolve(
        &Settings {
            threads: 4,
            mono: true,
            ..Default::default()
        },
        Path::new(&dir),
    )
    .unwrap();
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
    let probed = env
        .run(&s.ffprobe, &podcast_trim::ffargs::probe_args(&out))
        .unwrap();
    let (codec, _, d) = podcast_trim::bitrate::parse_probe(&probed);
    let d = d.unwrap();
    println!(
        "cleaned duration {d}, source {} {:?} -> opus {} kbps",
        ep.record.codec, ep.record.source_kbps, ep.record.opus_kbps
    );
    assert_eq!(codec, "opus");
    assert_eq!(ep.record.codec, "mp3");
    assert_eq!(
        ep.record.opus_kbps,
        podcast_trim::bitrate::cut_kbps("mp3", ep.record.source_kbps)
    );
    assert!((d - (ep.record.duration - 2.0)).abs() < 0.2);
    assert_eq!(&std::fs::read(&out).unwrap()[..4], b"OggS");
    pipeline::mark_delivered(&paths, 1, &mut ep);
    pipeline::regenerate(&env, &s, &paths, 1, &mut ep).unwrap();
    assert!(paths.cleaned(1, &ep).exists());
}
