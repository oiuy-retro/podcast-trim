//! Ad time ranges pasted back from the AI: parsing and validation.
//!
//! Accepted reply formats (anything else is rejected, never executed):
//! - One range per line: `HH:MM:SS.mmm-HH:MM:SS.mmm` (also `MM:SS.mmm`,
//!   or plain seconds like `12.5-40`). A line `NONE` means no ads.
//! - JSON: `{"ads":[{"start":"00:01:02.000","end":"00:02:03.500"}]}` where
//!   start/end may be strings in the same time format or numbers of seconds.
//!
//! Blank lines, lines starting with `#`, and Markdown code fences are ignored.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    /// Seconds from the start of the original episode.
    pub start: f64,
    pub end: f64,
}

/// Parse `HH:MM:SS.mmm`, `MM:SS.mmm`, or `SS.mmm` into seconds.
pub fn parse_time(text: &str) -> Result<f64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("empty time".into());
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return Err(format!("bad time `{t}`"));
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let ok = !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_digit() || (last && (c == '.' || c == ',')))
            && part.chars().filter(|c| *c == '.' || *c == ',').count() <= 1;
        if !ok {
            return Err(format!("bad time `{t}`"));
        }
        let value: f64 = part
            .replace(',', ".")
            .parse()
            .map_err(|_| format!("bad time `{t}`"))?;
        if i > 0 && value >= 60.0 {
            return Err(format!(
                "bad time `{t}`: minutes and seconds must be under 60"
            ));
        }
        total = total * 60.0 + value;
    }
    Ok(total)
}

pub fn format_time(seconds: f64) -> String {
    let ms_total = (seconds.max(0.0) * 1000.0).round() as u64;
    let ms = ms_total % 1000;
    let s = (ms_total / 1000) % 60;
    let m = (ms_total / 60_000) % 60;
    let h = ms_total / 3_600_000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

#[derive(Deserialize)]
struct JsonReply {
    ads: Vec<JsonCut>,
}

#[derive(Deserialize)]
struct JsonCut {
    start: serde_json::Value,
    end: serde_json::Value,
}

fn json_time(v: &serde_json::Value) -> Result<f64, String> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| "bad number".to_string()),
        serde_json::Value::String(s) => parse_time(s),
        _ => Err("start/end must be a time string or number of seconds".into()),
    }
}

/// Parse the AI reply into cuts (unvalidated, in reply order).
pub fn parse_reply(reply: &str) -> Result<Vec<Cut>, String> {
    let body: String = reply
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let parsed: JsonReply =
            serde_json::from_str(trimmed).map_err(|e| format!("JSON reply: {e}"))?;
        return parsed
            .ads
            .iter()
            .map(|c| {
                Ok(Cut {
                    start: json_time(&c.start)?,
                    end: json_time(&c.end)?,
                })
            })
            .collect();
    }
    let mut cuts = Vec::new();
    for (n, raw) in trimmed.lines().enumerate() {
        let line = raw.trim().trim_start_matches(['-', '*', ' ']).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.eq_ignore_ascii_case("none") {
            continue;
        }
        let line = line.replace(['\u{2013}', '\u{2014}'], "-");
        let (a, b) = line
            .split_once("-->")
            .or_else(|| line.split_once('-'))
            .ok_or_else(|| format!("line {}: expected START-END, got `{}`", n + 1, raw.trim()))?;
        let start = parse_time(a).map_err(|e| format!("line {}: {e}", n + 1))?;
        let end = parse_time(b).map_err(|e| format!("line {}: {e}", n + 1))?;
        cuts.push(Cut { start, end });
    }
    Ok(cuts)
}

/// Check cuts against the episode duration. Returns them sorted.
/// Rejects: end <= start, outside 0..=duration, overlaps, or a cut list that
/// would remove the whole episode.
pub fn validate(mut cuts: Vec<Cut>, duration: f64) -> Result<Vec<Cut>, String> {
    const SLACK: f64 = 0.5; // transcripts may round the last timestamp up
    for c in &cuts {
        if !c.start.is_finite() || !c.end.is_finite() {
            return Err("times must be finite".into());
        }
        if c.end <= c.start {
            return Err(format!(
                "{} - {}: end must be after start",
                format_time(c.start),
                format_time(c.end)
            ));
        }
        if c.start < 0.0 || c.end > duration + SLACK {
            return Err(format!(
                "{} - {} is outside the episode (length {})",
                format_time(c.start),
                format_time(c.end),
                format_time(duration)
            ));
        }
    }
    cuts.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    for w in cuts.windows(2) {
        if w[1].start < w[0].end {
            return Err(format!(
                "{} - {} overlaps {} - {}",
                format_time(w[0].start),
                format_time(w[0].end),
                format_time(w[1].start),
                format_time(w[1].end)
            ));
        }
    }
    for c in cuts.iter_mut() {
        c.end = c.end.min(duration);
    }
    if keep_segments(&cuts, duration).is_empty() {
        return Err("the cuts would remove the whole episode".into());
    }
    Ok(cuts)
}

/// Segments to keep, the complement of validated cuts. Slivers under 50 ms are dropped.
pub fn keep_segments(cuts: &[Cut], duration: f64) -> Vec<Cut> {
    let mut keep = Vec::new();
    let mut pos = 0.0;
    for c in cuts {
        if c.start - pos > 0.05 {
            keep.push(Cut {
                start: pos,
                end: c.start,
            });
        }
        pos = pos.max(c.end);
    }
    if duration - pos > 0.05 {
        keep.push(Cut {
            start: pos,
            end: duration,
        });
    }
    keep
}

pub fn removed_seconds(cuts: &[Cut]) -> f64 {
    cuts.iter().map(|c| c.end - c.start).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse_time("01:02:03.500").unwrap(), 3723.5);
        assert_eq!(parse_time("02:03.25").unwrap(), 123.25);
        assert_eq!(parse_time("42").unwrap(), 42.0);
        assert_eq!(parse_time("00:00:01,5").unwrap(), 1.5);
        assert!(parse_time("00:61:00").is_err());
        assert!(parse_time("1:2:3:4").is_err());
        assert!(parse_time("rm -rf").is_err());
        assert!(parse_time("1e3").is_err());
        assert_eq!(format_time(3723.5), "01:02:03.500");
    }

    #[test]
    fn lines_format() {
        let r = "```\n00:00:00.000-00:00:30.000\n# mid-roll\n00:10:00.000 - 00:11:30.500\n```";
        let c = parse_reply(r).unwrap();
        assert_eq!(
            c,
            vec![
                Cut {
                    start: 0.0,
                    end: 30.0
                },
                Cut {
                    start: 600.0,
                    end: 690.5
                }
            ]
        );
        assert!(parse_reply("NONE").unwrap().is_empty());
        assert_eq!(
            parse_reply("00:00:01.000 --> 00:00:02.000").unwrap().len(),
            1
        );
    }

    #[test]
    fn rejects_commands() {
        assert!(parse_reply("ffmpeg -i in.mp3 -af atrim out.mp3").is_err());
        assert!(parse_reply("00:00:01-00:00:02; del C:\\x").is_err());
    }

    #[test]
    fn json_format() {
        let r = r#"{"ads":[{"start":"00:01:00.000","end":"00:02:00.000"},{"start":5,"end":9.5}]}"#;
        let c = parse_reply(r).unwrap();
        assert_eq!(
            c[1],
            Cut {
                start: 5.0,
                end: 9.5
            }
        );
        assert!(parse_reply(r#"{"ads":[{"start":true,"end":1}]}"#).is_err());
    }

    #[test]
    fn validation() {
        let v = validate(
            vec![
                Cut {
                    start: 100.0,
                    end: 120.0,
                },
                Cut {
                    start: 10.0,
                    end: 20.0,
                },
            ],
            300.0,
        )
        .unwrap();
        assert_eq!(v[0].start, 10.0);
        assert!(validate(
            vec![Cut {
                start: 10.0,
                end: 5.0
            }],
            300.0
        )
        .is_err());
        assert!(validate(
            vec![Cut {
                start: 10.0,
                end: 400.0
            }],
            300.0
        )
        .is_err());
        assert!(validate(
            vec![
                Cut {
                    start: 10.0,
                    end: 50.0
                },
                Cut {
                    start: 40.0,
                    end: 60.0
                }
            ],
            300.0
        )
        .is_err());
        assert!(validate(
            vec![Cut {
                start: 0.0,
                end: 300.0
            }],
            300.0
        )
        .is_err());
        // slight overshoot is clamped
        let v = validate(
            vec![Cut {
                start: 290.0,
                end: 300.3,
            }],
            300.0,
        )
        .unwrap();
        assert_eq!(v[0].end, 300.0);
    }

    #[test]
    fn keep() {
        let k = keep_segments(
            &[
                Cut {
                    start: 0.0,
                    end: 30.0,
                },
                Cut {
                    start: 100.0,
                    end: 130.0,
                },
            ],
            200.0,
        );
        assert_eq!(
            k,
            vec![
                Cut {
                    start: 30.0,
                    end: 100.0
                },
                Cut {
                    start: 130.0,
                    end: 200.0
                }
            ]
        );
        assert_eq!(
            removed_seconds(&[Cut {
                start: 0.0,
                end: 30.0
            }]),
            30.0
        );
    }
}
