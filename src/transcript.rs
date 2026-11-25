//! whisper.cpp SRT output to a compact timestamped transcript.

use crate::cutlist::{format_time, parse_time};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub fn parse_srt(srt: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let text = srt.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let mut lines = block.lines().map(str::trim).filter(|l| !l.is_empty());
        let Some(first) = lines.next() else { continue };
        let timing = if first.contains("-->") {
            first
        } else {
            lines.next().unwrap_or("")
        };
        let Some((a, b)) = timing.split_once("-->") else {
            continue;
        };
        let (Ok(start), Ok(end)) = (parse_time(a), parse_time(b)) else {
            continue;
        };
        let words: Vec<&str> = lines.collect();
        let t = words.join(" ");
        if !t.is_empty() {
            out.push(Segment {
                start,
                end,
                text: t,
            });
        }
    }
    out
}

/// One line per segment: `[HH:MM:SS.mmm-HH:MM:SS.mmm] text`.
pub fn render(segments: &[Segment]) -> String {
    let mut s = String::new();
    for seg in segments {
        s.push_str(&format!(
            "[{}-{}] {}\n",
            format_time(seg.start),
            format_time(seg.end),
            seg.text
        ));
    }
    s
}

/// Read lines written by [`render`] back into segments. Other lines are skipped.
pub fn parse_rendered(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((times, words)) = rest.split_once(']') else {
            continue;
        };
        let Some((a, b)) = times.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) = (parse_time(a), parse_time(b)) else {
            continue;
        };
        let text = words.trim();
        if text.is_empty() {
            continue;
        }
        out.push(Segment {
            start,
            end,
            text: text.to_string(),
        });
    }
    out
}

/// Transcript words that overlap `[start, end]`, in time order.
/// A segment counts when any part of it falls inside the range, so a cut
/// that starts or ends mid-line still shows that line.
pub fn text_in_range(transcript: &str, start: f64, end: f64) -> String {
    let mut segs = parse_rendered(transcript);
    segs.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    segs.into_iter()
        .filter(|s| s.start < end && s.end > start)
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt() {
        let srt = "1\r\n00:00:00,000 --> 00:00:02,500\r\n Hello there.\r\n\r\n2\r\n00:00:02,500 --> 00:01:05,000\r\n This episode is\r\nbrought to you by\r\n\r\n";
        let s = parse_srt(srt);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].start, 2.5);
        assert_eq!(s[1].text, "This episode is brought to you by");
        assert_eq!(
            render(&s[..1]),
            "[00:00:00.000-00:00:02.500] Hello there.\n"
        );
    }

    #[test]
    fn words_inside_a_cut() {
        let transcript = "\
[00:00:00.000-00:00:02.500] Hello there.
[00:00:02.500-00:01:05.000] This episode is brought to you by Acme.
[00:01:05.000-00:01:08.000] Welcome back.
";
        assert_eq!(parse_rendered(transcript), parse_srt(
            "1\n00:00:00,000 --> 00:00:02,500\nHello there.\n\n2\n00:00:02,500 --> 00:01:05,000\nThis episode is brought to you by Acme.\n\n3\n00:01:05,000 --> 00:01:08,000\nWelcome back.\n\n"
        ));
        assert_eq!(
            text_in_range(transcript, 2.5, 65.0),
            "This episode is brought to you by Acme."
        );
        assert_eq!(
            text_in_range(transcript, 1.0, 3.0),
            "Hello there. This episode is brought to you by Acme."
        );
        assert_eq!(text_in_range(transcript, 100.0, 110.0), "");
        assert_eq!(text_in_range("not a transcript", 0.0, 10.0), "");
    }
}
