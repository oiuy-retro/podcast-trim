//! Reading progress out of ffmpeg's `-progress pipe:1` output and
//! whisper.cpp's `--print-progress` lines.

/// Seconds of output written so far, from one `-progress` line
/// (`out_time_us=`, `out_time_ms=` (also microseconds) or `out_time=HH:MM:SS.ff`).
pub fn ffmpeg_seconds(line: &str) -> Option<f64> {
    let line = line.trim();
    let (key, val) = line.split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => val
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .map(|v| v as f64 / 1e6),
        "out_time" => {
            let mut parts = val.trim().split(':');
            let h: f64 = parts.next()?.parse().ok()?;
            let m: f64 = parts.next()?.parse().ok()?;
            let s: f64 = parts.next()?.parse().ok()?;
            (h >= 0.0).then_some(h * 3600.0 + m * 60.0 + s)
        }
        _ => None,
    }
}

/// True for ffmpeg's final `progress=end` line.
pub fn ffmpeg_done(line: &str) -> bool {
    line.trim() == "progress=end"
}

/// Percent from whisper.cpp, e.g. `whisper_print_progress_callback: progress =  35%`.
pub fn whisper_percent(line: &str) -> Option<f64> {
    let at = line.find("progress =")?;
    let rest = line[at + "progress =".len()..].trim_start();
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let pct: f64 = num.parse().ok()?;
    rest[num.len()..]
        .trim_start()
        .starts_with('%')
        .then_some(pct.clamp(0.0, 100.0))
}

/// Fraction of `total` seconds, clamped to 0..=1.
pub fn fraction(done: f64, total: f64) -> f64 {
    if total <= 0.0 {
        0.0
    } else {
        (done / total).clamp(0.0, 1.0)
    }
}

/// How many blocks of `block` px with `gap` px between them fit in `width`,
/// and how many of those to fill for `frac`.
pub fn blocks(width: i32, block: i32, gap: i32, frac: f64) -> (i32, i32) {
    if width < block || block <= 0 {
        return (0, 0);
    }
    let cap = (width + gap) / (block + gap);
    let on = ((cap as f64) * frac.clamp(0.0, 1.0)).floor() as i32;
    (cap, on.min(cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_lines() {
        assert_eq!(ffmpeg_seconds("out_time_us=12500000"), Some(12.5));
        assert_eq!(ffmpeg_seconds("out_time_ms=2000000\r"), Some(2.0));
        assert_eq!(ffmpeg_seconds("out_time=00:01:02.500000"), Some(62.5));
        assert_eq!(ffmpeg_seconds("out_time_us=N/A"), None);
        assert_eq!(ffmpeg_seconds("out_time=-577014:32:22.775808"), None);
        assert_eq!(ffmpeg_seconds("bitrate=  64.0kbits/s"), None);
        assert!(ffmpeg_done("progress=end\r"));
        assert!(!ffmpeg_done("progress=continue"));
    }

    #[test]
    fn whisper_lines() {
        assert_eq!(
            whisper_percent("whisper_print_progress_callback: progress =  35%"),
            Some(35.0)
        );
        assert_eq!(whisper_percent("main: progress = 100%\r"), Some(100.0));
        assert_eq!(
            whisper_percent("[00:00:00.000 --> 00:00:05.000]  progress = good"),
            None
        );
        assert_eq!(whisper_percent("system_info: n_threads = 4"), None);
    }

    #[test]
    fn block_math() {
        assert_eq!(blocks(100, 8, 4, 0.0), (8, 0));
        assert_eq!(blocks(100, 8, 4, 0.5), (8, 4));
        assert_eq!(blocks(100, 8, 4, 1.0), (8, 8));
        assert_eq!(blocks(4, 8, 4, 1.0), (0, 0));
        assert_eq!(fraction(5.0, 0.0), 0.0);
        assert_eq!(fraction(50.0, 10.0), 1.0);
    }
}
