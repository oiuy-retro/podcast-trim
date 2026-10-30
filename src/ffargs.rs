//! Builds argument arrays for ffmpeg, ffprobe and whisper.cpp.
//! Arguments are passed straight to the process (no shell), and the only
//! values that come from the AI reply are validated numbers.

use crate::cutlist::{keep_segments, Cut};
use std::path::Path;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn num(x: f64) -> String {
    format!("{x:.3}")
}

/// ffmpeg args that keep everything except `cuts` (already validated) and
/// encode Ogg Opus (`.opus`), which is efficient for speech.
pub fn cut_args(
    input: &Path,
    output: &Path,
    cuts: &[Cut],
    duration: f64,
    bitrate_k: u32,
    mono: bool,
) -> Vec<String> {
    let keep = keep_segments(cuts, duration);
    let mut filter = String::new();
    for (i, k) in keep.iter().enumerate() {
        filter.push_str(&format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[k{i}];",
            num(k.start),
            num(k.end)
        ));
    }
    for i in 0..keep.len() {
        filter.push_str(&format!("[k{i}]"));
    }
    filter.push_str(&format!("concat=n={}:v=0:a=1[out]", keep.len()));
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        s(input),
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[out]".into(),
        "-map_metadata".into(),
        "0".into(),
        "-c:a".into(),
        "libopus".into(),
        "-b:a".into(),
        format!("{bitrate_k}k"),
        "-vbr".into(),
        "on".into(),
        "-application".into(),
        "audio".into(),
    ];
    if mono {
        args.extend(["-ac".into(), "1".into()]);
    }
    args.extend(["-f".into(), "ogg".into(), s(output)]);
    args
}

/// 16 kHz mono PCM WAV, the input whisper.cpp expects.
pub fn wav_args(input: &Path, wav: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        s(input),
        "-vn".into(),
        "-ar".into(),
        "16000".into(),
        "-ac".into(),
        "1".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        s(wav),
    ]
}

pub fn probe_args(input: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "format=duration".into(),
        "-of".into(),
        "default=noprint_wrappers=1:nokey=1".into(),
        s(input),
    ]
}

/// whisper.cpp CLI: writes `<out_base>.srt`.
pub fn whisper_args(
    model: &Path,
    wav: &Path,
    out_base: &Path,
    threads: u32,
    language: &str,
) -> Vec<String> {
    vec![
        "-m".into(),
        s(model),
        "-f".into(),
        s(wav),
        "-t".into(),
        threads.max(1).to_string(),
        "-l".into(),
        if language.trim().is_empty() {
            "auto".into()
        } else {
            language.trim().into()
        },
        "-osrt".into(),
        "-of".into(),
        s(out_base),
    ]
}

/// Human-readable command line for the approval screen and the record.
pub fn display(program: &str, args: &[String]) -> String {
    let mut out = program.to_string();
    for a in args {
        out.push(' ');
        if a.is_empty() || a.contains([' ', '[', ']', ';', '"']) {
            out.push('"');
            out.push_str(&a.replace('"', "\\\""));
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cut_filter() {
        let a = cut_args(
            &PathBuf::from("in file.mp3"),
            &PathBuf::from("out.opus"),
            &[
                Cut {
                    start: 0.0,
                    end: 30.0,
                },
                Cut {
                    start: 100.0,
                    end: 130.5,
                },
            ],
            200.0,
            64,
            true,
        );
        assert_eq!(a[4], "in file.mp3");
        let f = &a[6];
        assert_eq!(
            f,
            "[0:a]atrim=start=30.000:end=100.000,asetpts=PTS-STARTPTS[k0];\
             [0:a]atrim=start=130.500:end=200.000,asetpts=PTS-STARTPTS[k1];[k0][k1]concat=n=2:v=0:a=1[out]"
        );
        assert_eq!(a.last().unwrap(), "out.opus");
        assert!(a.windows(2).any(|w| w[0] == "-c:a" && w[1] == "libopus"));
        assert!(a.windows(2).any(|w| w[0] == "-ac" && w[1] == "1"));
        assert!(a.windows(2).any(|w| w[0] == "-f" && w[1] == "ogg"));
        assert!(a.contains(&"64k".to_string()));
    }

    #[test]
    fn no_cuts_is_one_segment() {
        let a = cut_args(
            &PathBuf::from("i"),
            &PathBuf::from("o"),
            &[],
            10.0,
            96,
            false,
        );
        assert!(a[6].ends_with("[k0]concat=n=1:v=0:a=1[out]"));
    }

    #[test]
    fn whisper() {
        let a = whisper_args(
            &PathBuf::from("m.bin"),
            &PathBuf::from("a.wav"),
            &PathBuf::from("a"),
            0,
            "",
        );
        assert_eq!(
            a,
            ["-m", "m.bin", "-f", "a.wav", "-t", "1", "-l", "auto", "-osrt", "-of", "a"]
        );
        assert!(display("ffmpeg", &["-i".into(), "a b".into()]).ends_with("\"a b\""));
    }
}
