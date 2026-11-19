//! Episode steps: download, transcribe, cut, deliver, regenerate, export.
//! Network and process calls go through `Env` so the logic is testable.

use crate::bitrate;
use crate::cutlist::{self, Cut};
use crate::ffargs;
use crate::progress;
use crate::rss;
use crate::store::{sha256_file, Channels, Episode, Library, Paths, Settings, Show, State};
use crate::transcript;
use std::fs;
use std::path::Path;

pub trait Env {
    fn fetch(&self, url: &str, dest: &Path) -> Result<(), String>;
    /// Run `program` with `args` (no shell) and return stdout.
    fn run(&self, program: &str, args: &[String]) -> Result<String, String>;
    /// Like `run`, but hands every stdout and stderr line to `line` as it
    /// arrives (for progress). The default just runs the program.
    fn run_watch(
        &self,
        program: &str,
        args: &[String],
        line: &mut dyn FnMut(&str),
    ) -> Result<String, String> {
        let _ = line;
        self.run(program, args)
    }
    /// Report what is happening and how far along it is (0..=1).
    fn progress(&self, _stage: &str, _fraction: f64) {}
}

/// Run ffmpeg and report `-progress` output as a fraction of `total` seconds,
/// mapped into `from..to`.
fn ffmpeg_watch(
    env: &dyn Env,
    program: &str,
    args: &[String],
    stage: &str,
    total: f64,
    (from, to): (f64, f64),
) -> Result<String, String> {
    env.progress(stage, from);
    env.run_watch(program, args, &mut |l| {
        if let Some(t) = progress::ffmpeg_seconds(l) {
            env.progress(stage, from + (to - from) * progress::fraction(t, total));
        } else if progress::ffmpeg_done(l) {
            env.progress(stage, to);
        }
    })
}

/// Add new episodes from a freshly fetched source feed. Returns how many were added.
pub fn merge_feed(show: &mut Show, feed: &rss::SourceFeed) -> usize {
    if show.title.is_empty() || show.title == show.feed_url {
        show.title = feed.title.clone();
    }
    let mut added = 0;
    for item in feed.items.iter().rev() {
        if show.episodes.iter().any(|e| e.guid == item.guid) {
            continue;
        }
        show.episodes.insert(
            0,
            Episode {
                guid: item.guid.clone(),
                title: item.title.clone(),
                pub_date: item.pub_date.clone(),
                source_url: item.url.clone(),
                state: State::New,
                record: Default::default(),
                cleaned_bytes: 0,
                served: 0,
                seen: false,
            },
        );
        added += 1;
    }
    added
}

/// Read duration, codec and bitrate of the original into the record.
fn probe(env: &dyn Env, s: &Settings, file: &Path, ep: &mut Episode) -> Result<(), String> {
    let out = env.run(&s.ffprobe, &ffargs::probe_args(file))?;
    let p = bitrate::parse_probe(&out);
    ep.record.duration = p
        .duration
        .ok_or_else(|| format!("ffprobe gave no duration: `{}`", out.trim()))?;
    ep.record.codec = p.codec;
    ep.record.source_kbps = p.kbps;
    ep.record.channels = p.channels;
    ep.record.identical = if p.channels == 2 {
        same_channels(env, s, file, ep.record.duration)
    } else {
        None
    };
    Ok(())
}

/// Are the left and right channels the same? `None` if the check failed.
fn same_channels(env: &dyn Env, s: &Settings, file: &Path, duration: f64) -> Option<bool> {
    let mut log = String::new();
    env.progress("Checking channels", 0.0);
    env.run_watch(
        &s.ffmpeg,
        &ffargs::stereo_check_args(file, duration),
        &mut |l| {
            if l.contains("Parsed_astats_") {
                log.push_str(l);
                log.push('\n');
            }
        },
    )
    .ok()?;
    let (side, mid) = bitrate::parse_side_mid(&log)?;
    Some(bitrate::identical_channels(side, mid))
}

/// Channels and bitrate for this episode's cut file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub kbps: u32,
    pub mono: bool,
    pub auto_kbps: bool,
    pub auto_channels: bool,
}

pub fn target(s: &Settings, ep: &Episode) -> Target {
    let r = &ep.record;
    let mono = match s.channels {
        Channels::Mono => true,
        Channels::Stereo => false,
        Channels::Auto => r.channels == 1 || r.identical == Some(true),
    };
    let kbps = if s.bitrate_k == 0 {
        bitrate::cut_kbps(&r.codec, r.source_kbps, mono)
    } else {
        s.bitrate_k
    };
    Target {
        kbps,
        mono,
        auto_kbps: s.bitrate_k == 0,
        auto_channels: s.channels == Channels::Auto,
    }
}

pub fn download(
    env: &dyn Env,
    s: &Settings,
    paths: &Paths,
    show: u32,
    ep: &mut Episode,
) -> Result<(), String> {
    let orig = paths.original(show, ep);
    fs::create_dir_all(paths.show_dir(show)).map_err(|e| e.to_string())?;
    env.fetch(&ep.source_url, &orig)?;
    let (hash, bytes) = sha256_file(&orig)?;
    ep.record.sha256 = hash;
    ep.record.source_bytes = bytes;
    probe(env, s, &orig, ep)?;
    ep.state = State::Downloaded;
    Ok(())
}

pub fn transcribe(
    env: &dyn Env,
    s: &Settings,
    paths: &Paths,
    show: u32,
    ep: &mut Episode,
) -> Result<(), String> {
    let orig = paths.original(show, ep);
    let wav = paths.work(show, ep, "wav");
    let base = paths.work(show, ep, "whisper");
    ffmpeg_watch(
        env,
        &s.ffmpeg,
        &ffargs::wav_args(&orig, &wav),
        "Converting for whisper.cpp",
        ep.record.duration,
        (0.0, 0.05),
    )?;
    let stage = "Transcribing";
    env.progress(stage, 0.05);
    let res = env.run_watch(
        &s.whisper_cli,
        &ffargs::whisper_args(
            Path::new(&s.whisper_model),
            &wav,
            &base,
            s.threads,
            &s.language,
        ),
        &mut |l| {
            if let Some(p) = progress::whisper_percent(l) {
                env.progress(stage, 0.05 + 0.95 * p / 100.0);
            }
        },
    );
    let _ = fs::remove_file(&wav);
    res?;
    let srt_path = std::path::PathBuf::from(format!("{}.srt", base.display()));
    let srt = fs::read_to_string(&srt_path)
        .map_err(|e| format!("whisper.cpp wrote no transcript: {e}"))?;
    let _ = fs::remove_file(&srt_path);
    let segs = transcript::parse_srt(&srt);
    if segs.is_empty() {
        return Err("The transcript is empty.".into());
    }
    ep.record.transcript = transcript::render(&segs);
    ep.state = State::Transcribed;
    Ok(())
}

/// Parse + validate an AI reply against this episode.
/// Check a reply for one episode: it must carry exactly one hash line and
/// that hash must be this episode's.
pub fn check_reply(ep: &Episode, reply: &str) -> Result<Vec<Cut>, String> {
    let want = ep
        .record
        .short_hash()
        .ok_or("this episode has no audio hash. Download it again.")?;
    let mut sections = cutlist::parse_sections(reply)?;
    if sections.len() > 1 {
        return Err(format!(
            "the reply has {} hash lines, but a reply for one episode has one",
            sections.len()
        ));
    }
    let s = sections.remove(0);
    if s.hash != want {
        return Err(format!(
            "the reply's hash {} does not match this episode's hash {want}. It may be for another episode.",
            s.hash
        ));
    }
    cutlist::validate(s.cuts, ep.record.duration)
}

/// Cuts for one episode from a reply covering several.
#[derive(Clone, Debug, PartialEq)]
pub struct BulkCut {
    pub show: u32,
    pub guid: String,
    pub title: String,
    pub cuts: Vec<Cut>,
}

/// Check a reply covering several episodes. Every hash must belong to a
/// transcribed episode in the library, and every episode's ranges must be
/// valid for it; otherwise nothing is accepted.
pub fn check_bulk(lib: &Library, reply: &str) -> Result<Vec<BulkCut>, String> {
    let sections = cutlist::parse_sections(reply)?;
    let mut out = Vec::new();
    for s in sections {
        let found = lib.shows.iter().find_map(|sh| {
            sh.episodes
                .iter()
                .find(|e| e.record.short_hash() == Some(s.hash.as_str()))
                .map(|e| (sh.id, e))
        });
        let Some((show, ep)) = found else {
            return Err(format!(
                "hash {} does not match any episode in the library",
                s.hash
            ));
        };
        if ep.state != State::Transcribed {
            return Err(format!(
                "hash {} is \"{}\", which is not waiting for cuts",
                s.hash, ep.title
            ));
        }
        let cuts = cutlist::validate(s.cuts, ep.record.duration)
            .map_err(|e| format!("\"{}\" (hash {}): {e}", ep.title, s.hash))?;
        out.push(BulkCut {
            show,
            guid: ep.guid.clone(),
            title: ep.title.clone(),
            cuts,
        });
    }
    Ok(out)
}

/// Cut with approved cuts. Deletes the original afterwards; the record keeps
/// what's needed to make the same file again.
pub fn apply_cuts(
    env: &dyn Env,
    s: &Settings,
    paths: &Paths,
    show: u32,
    ep: &mut Episode,
    cuts: Vec<Cut>,
) -> Result<(), String> {
    let orig = paths.original(show, ep);
    let out = paths.cleaned(show, ep);
    let t = target(s, ep);
    let args = ffargs::cut_args(&orig, &out, &cuts, ep.record.duration, t.kbps, t.mono);
    let kept = ep.record.duration - cuts.iter().map(|c| c.end - c.start).sum::<f64>();
    if let Err(e) = ffmpeg_watch(env, &s.ffmpeg, &args, "Cutting", kept, (0.0, 1.0)) {
        let _ = fs::remove_file(&out);
        return Err(e);
    }
    ep.cleaned_bytes = fs::metadata(&out)
        .map_err(|e| format!("ffmpeg wrote no file: {e}"))?
        .len();
    ep.record.cuts = cuts;
    ep.record.ffmpeg_args = args;
    ep.record.opus_kbps = t.kbps;
    ep.record.out_mono = t.mono;
    ep.state = State::Ready;
    let _ = fs::remove_file(&orig);
    Ok(())
}

/// A client has fetched the whole cleaned file: delete it, keep the record.
pub fn mark_delivered(paths: &Paths, show: u32, ep: &mut Episode) {
    let _ = fs::remove_file(paths.cleaned(show, ep));
    ep.served += 1;
    ep.state = State::Delivered;
}

/// Download the original again, check it is byte-for-byte the same file,
/// and rebuild the cleaned file from the stored cuts.
pub fn regenerate(
    env: &dyn Env,
    s: &Settings,
    paths: &Paths,
    show: u32,
    ep: &mut Episode,
) -> Result<(), String> {
    if ep.record.sha256.is_empty() || ep.record.transcript.is_empty() {
        return Err("No record for this episode; process it normally.".into());
    }
    let orig = paths.original(show, ep);
    fs::create_dir_all(paths.show_dir(show)).map_err(|e| e.to_string())?;
    env.fetch(&ep.source_url, &orig)?;
    let (hash, _) = sha256_file(&orig)?;
    if hash != ep.record.sha256 {
        // Publisher changed the file (often new dynamic ads). Old cuts don't apply.
        ep.record.sha256 = hash;
        ep.record.source_bytes = fs::metadata(&orig).map(|m| m.len()).unwrap_or(0);
        probe(env, s, &orig, ep)?;
        ep.record.transcript.clear();
        ep.record.cuts.clear();
        ep.record.ffmpeg_args.clear();
        ep.state = State::Downloaded;
        return Err("The publisher's file has changed since it was trimmed, so the saved cuts no longer apply. It has been downloaded again; transcribe it and ask the AI again.".into());
    }
    let cuts = ep.record.cuts.clone();
    apply_cuts(env, s, paths, show, ep, cuts)
}

pub fn safe_name(title: &str) -> String {
    let mut n: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    n = n.trim().trim_end_matches('.').to_string();
    if n.is_empty() {
        n = "episode".into();
    }
    n.chars().take(120).collect()
}

pub fn export(
    paths: &Paths,
    show: u32,
    ep: &Episode,
    dir: &Path,
) -> Result<std::path::PathBuf, String> {
    let src = paths.cleaned(show, ep);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{}.opus", safe_name(&ep.title)));
    fs::copy(&src, &dest).map_err(|e| format!("{}: {e}", src.display()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Fake env: "downloads" fixed bytes, ffmpeg writes its last arg, ffprobe says 100 s.
    struct Fake {
        body: RefCell<Vec<u8>>,
        calls: RefCell<Vec<(String, Vec<String>)>>,
        seen: RefCell<Vec<(String, f64)>>,
        /// ffprobe output, and the side level the fake stereo check reports.
        probe: RefCell<String>,
        side_db: RefCell<String>,
    }

    impl Env for Fake {
        fn fetch(&self, _url: &str, dest: &Path) -> Result<(), String> {
            fs::write(dest, &*self.body.borrow()).map_err(|e| e.to_string())
        }
        fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
            self.calls
                .borrow_mut()
                .push((program.into(), args.to_vec()));
            match program {
                "ffprobe" => Ok(self.probe.borrow().clone()),
                "ffmpeg" if args.last().unwrap() == "-" => Ok(String::new()),
                "ffmpeg" => {
                    fs::write(args.last().unwrap(), b"cleaned").unwrap();
                    Ok(String::new())
                }
                "whisper" => {
                    let base = &args[args.iter().position(|a| a == "-of").unwrap() + 1];
                    fs::write(format!("{base}.srt"), "1\n00:00:00,000 --> 00:00:10,000\nAd here\n\n2\n00:00:10,000 --> 00:01:40,000\nShow\n").unwrap();
                    Ok(String::new())
                }
                _ => Err("unknown".into()),
            }
        }
        fn run_watch(
            &self,
            program: &str,
            args: &[String],
            line: &mut dyn FnMut(&str),
        ) -> Result<String, String> {
            let out = self.run(program, args)?;
            if args.last().unwrap() == "-" {
                line(&format!(
                    "[Parsed_astats_3 @ 0x1] RMS level dB: {}",
                    self.side_db.borrow()
                ));
                line("[Parsed_astats_5 @ 0x2] RMS level dB: -20.0");
                return Ok(out);
            }
            for l in [
                "out_time_us=40000000",
                "whisper: progress =  50%",
                "progress=end",
            ] {
                line(l);
            }
            Ok(out)
        }
        fn progress(&self, stage: &str, fraction: f64) {
            self.seen.borrow_mut().push((stage.into(), fraction));
        }
    }

    fn setup(tag: &str) -> (Fake, Settings, Paths, Episode) {
        let root = std::env::temp_dir().join(format!("pt-pipe-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let s = Settings {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            whisper_cli: "whisper".into(),
            ..Default::default()
        };
        let ep = Episode {
            guid: "g".into(),
            title: "Ep: 1?".into(),
            pub_date: String::new(),
            source_url: "http://x/1.mp3".into(),
            state: State::New,
            record: Default::default(),
            cleaned_bytes: 0,
            served: 0,
            seen: false,
        };
        (
            Fake {
                body: RefCell::new(b"original".to_vec()),
                calls: RefCell::new(vec![]),
                seen: RefCell::new(vec![]),
                probe: RefCell::new("100.0\n".into()),
                side_db: RefCell::new("-inf".into()),
            },
            s,
            Paths::new(root),
            ep,
        )
    }

    #[test]
    fn full_cycle_and_regenerate() {
        let (env, s, paths, mut ep) = setup("cycle");
        download(&env, &s, &paths, 1, &mut ep).unwrap();
        assert_eq!(ep.record.sha256, crate::store::sha256_hex(b"original"));
        assert_eq!(ep.record.duration, 100.0);
        transcribe(&env, &s, &paths, 1, &mut ep).unwrap();
        assert!(ep
            .record
            .transcript
            .starts_with("[00:00:00.000-00:00:10.000] Ad here"));
        let h = format!("hash: {}\n", &ep.record.sha256[..12]);
        let cuts = check_reply(&ep, &format!("{h}00:00:00.000-00:00:10.000")).unwrap();
        assert!(check_reply(&ep, &format!("{h}00:00:00.000-00:05:00.000")).is_err());
        apply_cuts(&env, &s, &paths, 1, &mut ep, cuts).unwrap();
        assert_eq!(ep.state, State::Ready);
        assert!(paths.cleaned(1, &ep).exists());
        assert!(!paths.original(1, &ep).exists());
        let first_args = ep.record.ffmpeg_args.clone();

        let out = export(&paths, 1, &ep, &paths.root.join("export")).unwrap();
        assert!(out.ends_with("Ep_ 1_.opus"));

        mark_delivered(&paths, 1, &mut ep);
        assert_eq!(ep.state, State::Delivered);
        assert!(!paths.cleaned(1, &ep).exists());

        // Regenerate: no whisper call, same ffmpeg args.
        let whisper_before = env
            .calls
            .borrow()
            .iter()
            .filter(|c| c.0 == "whisper")
            .count();
        regenerate(&env, &s, &paths, 1, &mut ep).unwrap();
        assert_eq!(ep.state, State::Ready);
        assert_eq!(ep.record.ffmpeg_args, first_args);
        assert_eq!(
            env.calls
                .borrow()
                .iter()
                .filter(|c| c.0 == "whisper")
                .count(),
            whisper_before
        );
        assert!(paths.cleaned(1, &ep).exists());
    }

    #[test]
    fn reports_progress() {
        let (env, s, paths, mut ep) = setup("progress");
        download(&env, &s, &paths, 1, &mut ep).unwrap();
        transcribe(&env, &s, &paths, 1, &mut ep).unwrap();
        let seen = env.seen.borrow().clone();
        // 40 s of 100 s converted is 40% of the 5% conversion share.
        assert!(seen
            .iter()
            .any(|(st, f)| st.starts_with("Converting") && (*f - 0.02).abs() < 1e-9));
        assert!(seen
            .iter()
            .any(|(st, f)| st == "Transcribing" && (*f - 0.525).abs() < 1e-9));
        env.seen.borrow_mut().clear();
        let h = format!("hash: {}\n", &ep.record.sha256[..12]);
        let cuts = check_reply(&ep, &format!("{h}00:00:00.000-00:00:20.000")).unwrap();
        apply_cuts(&env, &s, &paths, 1, &mut ep, cuts).unwrap();
        // 40 s of the 80 s kept.
        assert!(env
            .seen
            .borrow()
            .iter()
            .any(|(st, f)| st == "Cutting" && (*f - 0.5).abs() < 1e-9));
        assert_eq!(env.seen.borrow().last().unwrap().1, 1.0);
    }

    #[test]
    fn auto_channels_and_bitrate() {
        let (env, mut s, paths, mut ep) = setup("channels");
        *env.probe.borrow_mut() = "[STREAM]\ncodec_name=mp3\nchannels=2\nbit_rate=55000\n[/STREAM]\n[FORMAT]\nduration=100.0\n[/FORMAT]\n".into();
        // Identical channels: mono, mono tier.
        download(&env, &s, &paths, 1, &mut ep).unwrap();
        assert_eq!(ep.record.identical, Some(true));
        let t = target(&s, &ep);
        assert_eq!((t.kbps, t.mono), (64, true));
        // Real stereo.
        *env.side_db.borrow_mut() = "-31.0".into();
        download(&env, &s, &paths, 1, &mut ep).unwrap();
        assert_eq!(ep.record.identical, Some(false));
        assert_eq!((target(&s, &ep).kbps, target(&s, &ep).mono), (96, false));
        // Overrides. Mono still uses Auto's floor; a typed bitrate does not.
        s.channels = Channels::Mono;
        assert_eq!((target(&s, &ep).kbps, target(&s, &ep).mono), (64, true));
        s.bitrate_k = 64;
        s.channels = Channels::Stereo;
        let t = target(&s, &ep);
        assert_eq!(
            (t.kbps, t.mono, t.auto_kbps, t.auto_channels),
            (64, false, false, false)
        );
        transcribe(&env, &s, &paths, 1, &mut ep).unwrap();
        apply_cuts(&env, &s, &paths, 1, &mut ep, vec![]).unwrap();
        assert_eq!((ep.record.opus_kbps, ep.record.out_mono), (64, false));
        assert!(!ep.record.ffmpeg_args.contains(&"-ac".to_string()));
    }

    #[test]
    fn regenerate_rejects_changed_file() {
        let (env, s, paths, mut ep) = setup("changed");
        download(&env, &s, &paths, 1, &mut ep).unwrap();
        transcribe(&env, &s, &paths, 1, &mut ep).unwrap();
        apply_cuts(&env, &s, &paths, 1, &mut ep, vec![]).unwrap();
        mark_delivered(&paths, 1, &mut ep);
        *env.body.borrow_mut() = b"new dynamic ads".to_vec();
        let err = regenerate(&env, &s, &paths, 1, &mut ep).unwrap_err();
        assert!(err.contains("changed"));
        assert_eq!(ep.state, State::Downloaded);
        assert!(ep.record.cuts.is_empty() && ep.record.transcript.is_empty());
    }

    fn transcribed(guid: &str, hash: &str, duration: f64) -> Episode {
        Episode {
            guid: guid.into(),
            title: format!("Ep {guid}"),
            pub_date: String::new(),
            source_url: String::new(),
            state: State::Transcribed,
            record: crate::store::Record {
                sha256: hash.into(),
                duration,
                ..Default::default()
            },
            cleaned_bytes: 0,
            served: 0,
            seen: false,
        }
    }

    #[test]
    fn reply_hash_must_match() {
        let ep = transcribed("a", &"1a2b3c4d5e6f".repeat(5), 300.0);
        let ok = check_reply(&ep, "hash: 1a2b3c4d5e6f\n00:00:10.000-00:00:20.000").unwrap();
        assert_eq!(ok, vec![Cut { start: 10.0, end: 20.0 }]);
        assert!(check_reply(&ep, "hash: 1a2b3c4d5e6f\nNONE").unwrap().is_empty());
        let e = check_reply(&ep, "00:00:10.000-00:00:20.000").unwrap_err();
        assert!(e.contains("no hash line"), "{e}");
        let e = check_reply(&ep, "hash: ffffffffffff\n00:00:10.000-00:00:20.000").unwrap_err();
        assert!(e.contains("does not match this episode's hash 1a2b3c4d5e6f"), "{e}");
        let e = check_reply(&ep, "hash: 1a2b3c4d5e6f\nNONE\nhash: ffffffffffff\nNONE").unwrap_err();
        assert!(e.contains("2 hash lines"), "{e}");
        let mut no_hash = ep.clone();
        no_hash.record.sha256.clear();
        assert!(check_reply(&no_hash, "hash: 1a2b3c4d5e6f\nNONE").unwrap_err().contains("no audio hash"));
    }

    #[test]
    fn bulk_reply() {
        let mut lib = Library::default();
        let a = lib.add_show("A".into(), "u".into());
        let b = lib.add_show("B".into(), "v".into());
        lib.show_mut(a).unwrap().episodes = vec![
            transcribed("a1", &"a".repeat(64), 600.0),
            transcribed("a2", &"b".repeat(64), 600.0),
        ];
        let mut ready = transcribed("b2", &"d".repeat(64), 600.0);
        ready.state = State::Ready;
        lib.show_mut(b).unwrap().episodes = vec![transcribed("b1", &"c".repeat(64), 100.0), ready];
        let reply = "hash: cccccccccccc\n00:00:00.000-00:00:30.000\n\
                     hash: aaaaaaaaaaaa\nNONE\n\
                     hash: bbbbbbbbbbbb\n00:05:00.000-00:06:00.000\n00:01:00.000-00:02:00.000\n";
        let got = check_bulk(&lib, reply).unwrap();
        assert_eq!(
            got.iter().map(|c| (c.show, c.guid.as_str(), c.cuts.len())).collect::<Vec<_>>(),
            [(b, "b1", 1), (a, "a1", 0), (a, "a2", 2)]
        );
        // Sorted for cutting.
        assert_eq!(got[2].cuts[0].start, 60.0);
        // Unknown hash: nothing is accepted.
        let e = check_bulk(&lib, &format!("{reply}hash: 999999999999\nNONE")).unwrap_err();
        assert!(e.contains("999999999999 does not match any episode"), "{e}");
        // A hash that belongs to an episode that isn't waiting for cuts.
        let e = check_bulk(&lib, "hash: dddddddddddd\nNONE").unwrap_err();
        assert!(e.contains("not waiting for cuts"), "{e}");
        // Ranges checked against the matching episode's own length (b1 is 100 s).
        let e = check_bulk(&lib, "hash: aaaaaaaaaaaa\n00:03:00.000-00:04:00.000\nhash: cccccccccccc\n00:03:00.000-00:04:00.000").unwrap_err();
        assert!(e.starts_with("\"Ep b1\" (hash cccccccccccc)"), "{e}");
        assert!(check_bulk(&lib, "00:00:00.000-00:00:30.000").is_err());
    }

    #[test]
    fn merge() {
        let mut show = Show {
            id: 1,
            title: "u".into(),
            feed_url: "u".into(),
            token: "t".into(),
            episodes: vec![],
            seen_rule_done: false,
        };
        let feed = rss::SourceFeed {
            title: "Real".into(),
            items: vec![
                rss::SourceItem {
                    guid: "2".into(),
                    title: "Two".into(),
                    url: "a".into(),
                    pub_date: String::new(),
                },
                rss::SourceItem {
                    guid: "1".into(),
                    title: "One".into(),
                    url: "b".into(),
                    pub_date: String::new(),
                },
            ],
        };
        assert_eq!(merge_feed(&mut show, &feed), 2);
        assert_eq!(merge_feed(&mut show, &feed), 0);
        assert_eq!(show.title, "Real");
        assert_eq!(show.episodes[0].guid, "2");
    }
}
