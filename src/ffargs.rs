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
    args.extend(progress_args());
    args.extend(["-f".into(), "ogg".into(), s(output)]);
    args
}

/// 16 kHz mono PCM WAV, the input whisper.cpp expects.
pub fn wav_args(input: &Path, wav: &Path) -> Vec<String> {
    [
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
    ]
    .into_iter()
    .chain(progress_args())
    .chain([s(wav)])
    .collect()
}

/// Quick fake-stereo check: three 10 s samples (at 15%, 50% and 85%) are
/// joined, then the RMS level of the side (L-R) and mid (L+R) signals is
/// measured with astats. Fast seeking keeps this to about 30 s of decoding.
pub fn stereo_check_args(input: &Path, duration: f64) -> Vec<String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into()];
    let starts: Vec<f64> = if duration > 40.0 {
        [0.15, 0.5, 0.85]
            .iter()
            .map(|f| duration * f - 5.0)
            .collect()
    } else {
        vec![0.0]
    };
    for st in &starts {
        args.extend([
            "-ss".into(),
            num(st.max(0.0)),
            "-t".into(),
            "10".into(),
            "-i".into(),
            s(input),
        ]);
    }
    let n = starts.len();
    let ins: String = (0..n).map(|i| format!("[{i}:a]")).collect();
    let stats = "astats=measure_perchannel=none:measure_overall=RMS_level";
    args.extend([
        "-filter_complex".into(),
        format!(
            "{ins}concat=n={n}:v=0:a=1,asplit[x][y];\
             [x]pan=mono|c0=0.5*c0-0.5*c1,{stats}[side];\
             [y]pan=mono|c0=0.5*c0+0.5*c1,{stats}[mid]"
        ),
        "-map".into(),
        "[side]".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
        "-map".into(),
        "[mid]".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    args
}

/// Machine-readable progress on stdout (`out_time_us=...`), no stats on stderr.
fn progress_args() -> [String; 3] {
    ["-progress".into(), "pipe:1".into(), "-nostats".into()]
}

pub fn probe_args(input: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        "a:0".into(),
        "-show_entries".into(),
        "stream=codec_name,bit_rate,channels:format=duration,bit_rate".into(),
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
        "-pp".into(),
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
        assert!(a
            .windows(2)
            .any(|w| w[0] == "-progress" && w[1] == "pipe:1"));
        let w = wav_args(&PathBuf::from("i"), &PathBuf::from("o.wav"));
        assert_eq!(w.last().unwrap(), "o.wav");
        assert!(w.contains(&"-nostats".to_string()));
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
    fn stereo_check() {
        let a = stereo_check_args(&PathBuf::from("x.mp3"), 1000.0);
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 3);
        assert!(a.contains(&"145.000".to_string()) && a.contains(&"845.000".to_string()));
        let g = &a[a.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(g.starts_with("[0:a][1:a][2:a]concat=n=3"));
        assert_eq!(
            stereo_check_args(&PathBuf::from("x"), 20.0)
                .iter()
                .filter(|x| *x == "-i")
                .count(),
            1
        );
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
            ["-m", "m.bin", "-f", "a.wav", "-t", "1", "-l", "auto", "-pp", "-osrt", "-of", "a"]
        );
        assert!(display("ffmpeg", &["-i".into(), "a b".into()]).ends_with("\"a b\""));
    }
}
