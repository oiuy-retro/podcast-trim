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
}
