//! Opus bitrate from the source codec and its average bitrate.
//! Cutting always re-encodes, so an Opus source is re-encoded at its own
//! bitrate, rounded up to the next tier. Auto never goes under
//! [`STEREO_FLOOR`] / [`MONO_FLOOR`]: the old low tiers were fine at normal
//! speed and fell apart when a podcast app played the file at 2× or faster.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecClass {
    Opus,
    Lossless,
    Aac,
    Vorbis,
    Mp3,
    Ac3,
    Dts,
    Other,
}

pub fn classify(codec: &str) -> CodecClass {
    match codec.to_ascii_lowercase().as_str() {
        "opus" => CodecClass::Opus,
        "flac" | "alac" | "ape" | "wavpack" | "tta" | "truehd" | "mlp" | "pcm_s16le"
        | "pcm_s16be" | "pcm_s24le" | "pcm_s24be" | "pcm_s32le" | "pcm_s32be" | "pcm_f32le"
        | "pcm_f64le" | "pcm_u8" | "pcm_s8" | "pcm_s16le_planar" | "pcm_s24le_planar"
        | "dsd_lsbf" | "dsd_msbf" | "dsd_lsbf_planar" | "dsd_msbf_planar" => CodecClass::Lossless,
        "aac" | "mp4a" | "aac_latm" => CodecClass::Aac,
        "vorbis" | "libvorbis" => CodecClass::Vorbis,
        "mp3" | "mp2" | "mp1" | "wma" | "wmav1" | "wmav2" | "wmapro" => CodecClass::Mp3,
        "ac3" | "eac3" | "ac-3" => CodecClass::Ac3,
        "dts" | "dca" | "dts-hd" => CodecClass::Dts,
        _ => CodecClass::Other,
    }
}

/// Stereo Auto never encodes below this. A 2× time-stretch needs the
/// fuller signal; 32–64 kbps speech tiers did not survive it.
const STEREO_FLOOR: u32 = 96;
/// Mono Auto floor, including stereo whose channels match and are folded
/// to one channel.
const MONO_FLOOR: u32 = 64;

/// Stereo Opus tier for a source. At or below 128k the tier is
/// [`STEREO_FLOOR`]. Above that, the rate steps up with the source.
/// `None` = Opus source, which is handled in [`cut_kbps`].
fn stereo_tier(codec: &str, source_kbps: Option<u32>) -> Option<u32> {
    match classify(codec) {
        CodecClass::Opus => None,
        CodecClass::Lossless | CodecClass::Dts => Some(192),
        CodecClass::Aac | CodecClass::Vorbis => Some(match source_kbps.unwrap_or(128) {
            0..=128 => STEREO_FLOOR,
            129..=192 => 128,
            _ => 160,
        }),
        CodecClass::Mp3 | CodecClass::Other => Some(match source_kbps.unwrap_or(128) {
            0..=128 => STEREO_FLOOR,
            129..=192 => 128,
            193..=256 => 160,
            _ => 192,
        }),
        CodecClass::Ac3 => Some(match source_kbps.unwrap_or(384) {
            0..=192 => 128,
            193..=384 => 160,
            _ => 192,
        }),
    }
}

/// Opus tiers an Opus source is rounded up to.
const OPUS_TIERS: [u32; 8] = [24, 32, 48, 64, 96, 128, 160, 192];
/// Mono tiers: about 60% of the stereo tier, rounded up, at least 24k.
const MONO_TIERS: [u32; 10] = [24, 32, 40, 48, 56, 64, 80, 96, 112, 128];

fn round_up(k: u32, tiers: &[u32]) -> u32 {
    *tiers
        .iter()
        .find(|t| **t >= k)
        .unwrap_or(tiers.last().unwrap())
}

/// Mono bitrate for a stereo tier: 32 -> 24, 48 -> 32, 64 -> 40, 96 -> 64,
/// 128 -> 80, 160 -> 96, 192 -> 128.
pub fn mono_of(stereo: u32) -> u32 {
    round_up((stereo * 6).div_ceil(10), &MONO_TIERS)
}

/// Bitrate for re-encoding the cut episode (cutting always re-encodes, so
/// an Opus source keeps its own tier instead of being copied). The result
/// is then raised to [`MONO_FLOOR`] or [`STEREO_FLOOR`].
pub fn cut_kbps(codec: &str, source_kbps: Option<u32>, mono: bool) -> u32 {
    let rate = match stereo_tier(codec, source_kbps) {
        Some(k) => {
            if mono {
                mono_of(k)
            } else {
                k
            }
        }
        None => match source_kbps {
            Some(k) if mono => round_up(k, &MONO_TIERS).min(mono_of(round_up(k, &OPUS_TIERS))),
            Some(k) => round_up(k, &OPUS_TIERS),
            None => STEREO_FLOOR,
        },
    };
    if mono {
        rate.max(MONO_FLOOR)
    } else {
        rate.max(STEREO_FLOOR)
    }
}

/// What ffprobe said about the original.
#[derive(Debug, Default, PartialEq)]
pub struct Probe {
    pub codec: String,
    pub kbps: Option<u32>,
    pub channels: u32,
    pub duration: Option<f64>,
}

/// Parse ffprobe `key=value` output (stream codec_name / bit_rate / channels
/// and format duration / bit_rate).
pub fn parse_probe(out: &str) -> Probe {
    let mut p = Probe::default();
    let (mut stream_br, mut format_br) = (None, None);
    let mut in_format = false;
    for l in out.lines() {
        let l = l.trim();
        match l {
            "[FORMAT]" => in_format = true,
            "[STREAM]" | "[/FORMAT]" => in_format = false,
            _ => {}
        }
        let Some((k, v)) = l.split_once('=') else {
            // Older callers: a bare duration.
            if let Ok(d) = l.parse::<f64>() {
                p.duration = Some(d);
            }
            continue;
        };
        let num = v.trim().parse::<f64>().ok();
        match k {
            "codec_name" => p.codec = v.trim().to_string(),
            "channels" => p.channels = num.unwrap_or(0.0) as u32,
            "duration" if in_format || p.duration.is_none() => p.duration = num.or(p.duration),
            "bit_rate" if in_format => format_br = num,
            "bit_rate" => stream_br = num,
            _ => {}
        }
    }
    p.kbps = stream_br
        .or(format_br)
        .filter(|b| *b > 0.0)
        .map(|b| (b / 1000.0).round() as u32);
    p
}

/// How far below the mid (L+R) the side (L-R) signal must be for the two
/// channels to count as the same. Real stereo is usually 6-20 dB below;
/// mono stored as stereo (even after joint-stereo MP3) is 40 dB or more.
pub const IDENTICAL_DB: f64 = 30.0;

/// Fake-stereo decision from the RMS levels (dBFS) of side and mid.
/// `None` level = silence (-inf).
pub fn identical_channels(side_db: Option<f64>, mid_db: Option<f64>) -> bool {
    match (side_db, mid_db) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(s), Some(m)) => m - s >= IDENTICAL_DB,
    }
}

/// Pull the overall RMS levels out of ffmpeg's astats log. The filter graph
/// puts the side analysis first, so the lower `Parsed_astats_N` is side.
pub fn parse_side_mid(stderr: &str) -> Option<(Option<f64>, Option<f64>)> {
    let mut found: Vec<(u32, Option<f64>)> = Vec::new();
    for l in stderr.lines() {
        let Some(at) = l.find("Parsed_astats_") else {
            continue;
        };
        let Some(pos) = l.find("RMS level dB:") else {
            continue;
        };
        let n: u32 = l[at + 14..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;
        let v = l[pos + 13..].trim();
        let db = v.parse::<f64>().ok().filter(|d| d.is_finite());
        found.retain(|(k, _)| *k != n);
        found.push((n, db));
    }
    if found.len() != 2 {
        return None;
    }
    found.sort_by_key(|f| f.0);
    Some((found[0].1, found[1].1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_tiers() {
        // At or below 128k, stereo stays on the 2× floor.
        assert_eq!(cut_kbps("mp3", Some(32), false), 96);
        assert_eq!(cut_kbps("mp3", Some(40), false), 96);
        assert_eq!(cut_kbps("mp3", Some(55), false), 96);
        assert_eq!(cut_kbps("mp3", Some(64), false), 96);
        assert_eq!(cut_kbps("aac", Some(96), false), 96);
        assert_eq!(cut_kbps("mp3", Some(128), false), 96);
        // Above 128k the rate still steps up with the source.
        assert_eq!(cut_kbps("mp3", Some(192), false), 128);
        assert_eq!(cut_kbps("mp3", Some(256), false), 160);
        assert_eq!(cut_kbps("mp3", Some(320), false), 192);
        assert_eq!(cut_kbps("aac", Some(256), false), 160);
        assert_eq!(cut_kbps("wmav2", Some(160), false), 128);
        assert_eq!(cut_kbps("flac", Some(900), false), 192);
        assert_eq!(cut_kbps("ac3", Some(192), false), 128);
        assert_eq!(cut_kbps("dts", Some(1509), false), 192);
        assert_eq!(cut_kbps("mp3", None, false), 96);
    }

    #[test]
    fn mono_tiers() {
        assert_eq!(mono_of(32), 24);
        assert_eq!(mono_of(48), 32);
        assert_eq!(mono_of(64), 40);
        assert_eq!(mono_of(96), 64);
        assert_eq!(mono_of(128), 80);
        assert_eq!(mono_of(160), 96);
        assert_eq!(mono_of(192), 128);
        assert_eq!(cut_kbps("mp3", Some(55), true), 64);
        assert_eq!(cut_kbps("mp3", Some(24), true), 64);
        assert_eq!(cut_kbps("mp3", Some(192), true), 80);
        assert_eq!(cut_kbps("mp3", Some(320), true), 128);
    }

    #[test]
    fn opus_keeps_its_tier() {
        assert_eq!(cut_kbps("opus", Some(32), false), 96);
        assert_eq!(cut_kbps("opus", Some(40), false), 96);
        assert_eq!(cut_kbps("opus", Some(128), false), 128);
        assert_eq!(cut_kbps("opus", Some(300), false), 192);
        assert_eq!(cut_kbps("opus", None, false), 96);
        assert_eq!(cut_kbps("opus", Some(64), true), 64);
        assert_eq!(cut_kbps("opus", Some(24), true), 64);
        assert_eq!(cut_kbps("opus", Some(128), true), 80);
    }

    #[test]
    fn speed_playback_floor() {
        for k in [24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 256, 320] {
            for c in ["mp3", "aac", "vorbis", "opus"] {
                assert!(cut_kbps(c, Some(k), false) >= STEREO_FLOOR, "{c} {k} stereo");
                assert!(cut_kbps(c, Some(k), true) >= MONO_FLOOR, "{c} {k} mono");
            }
        }
    }

    #[test]
    fn probe_output() {
        let out = "[STREAM]\ncodec_name=mp3\nchannels=2\nbit_rate=128000\n[/STREAM]\n[FORMAT]\nduration=221.672000\nbit_rate=128115\n[/FORMAT]\n";
        assert_eq!(
            parse_probe(out),
            Probe {
                codec: "mp3".into(),
                kbps: Some(128),
                channels: 2,
                duration: Some(221.672)
            }
        );
        let p = parse_probe("[STREAM]\ncodec_name=opus\nbit_rate=N/A\n[/STREAM]\n[FORMAT]\nduration=10.0\nbit_rate=48000\n[/FORMAT]\n");
        assert_eq!(p.kbps, Some(48));
        assert_eq!(parse_probe("100.0\n").duration, Some(100.0));
    }

    #[test]
    fn fake_stereo_decision() {
        assert!(identical_channels(Some(-62.0), Some(-20.0)));
        assert!(identical_channels(None, Some(-20.0)));
        assert!(!identical_channels(Some(-32.0), Some(-20.0)));
        assert!(!identical_channels(Some(-49.0), Some(-20.0)));
        assert!(identical_channels(Some(-50.0), Some(-20.0)));
        assert!(!identical_channels(Some(-40.0), None));
        let log = "[Parsed_astats_5 @ 0x1] Overall\n[Parsed_astats_5 @ 0x1] RMS level dB: -21.5\n[Parsed_astats_3 @ 0x2] Overall\n[Parsed_astats_3 @ 0x2] RMS level dB: -inf\n";
        assert_eq!(parse_side_mid(log), Some((None, Some(-21.5))));
        assert_eq!(parse_side_mid("nothing"), None);
    }
}
