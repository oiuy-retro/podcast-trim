//! Opus bitrate from the source codec and its average bitrate.
//! Same mapping as Opus Convert. The one difference: Opus Convert copies
//! Opus sources untouched, but cutting always re-encodes, so an Opus source
//! is re-encoded at its own bitrate, rounded up to the next Opus tier.

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

/// Target Opus bitrate in kbps, or `None` when the audio should be copied.
pub fn opus_kbps(codec: &str, source_kbps: Option<u32>) -> Option<u32> {
    match classify(codec) {
        CodecClass::Opus => None,
        CodecClass::Lossless => Some(192),
        CodecClass::Aac | CodecClass::Vorbis => Some(aac_ladder(source_kbps.unwrap_or(128))),
        CodecClass::Mp3 | CodecClass::Other => Some(mp3_ladder(source_kbps.unwrap_or(192))),
        CodecClass::Ac3 => Some(ac3_ladder(source_kbps.unwrap_or(384))),
        CodecClass::Dts => Some(192),
    }
}

fn mp3_ladder(kbps: u32) -> u32 {
    if kbps <= 128 {
        96
    } else if kbps <= 192 {
        128
    } else if kbps <= 256 {
        160
    } else {
        192
    }
}

fn aac_ladder(kbps: u32) -> u32 {
    if kbps <= 128 {
        96
    } else if kbps <= 192 {
        128
    } else {
        160
    }
}

fn ac3_ladder(kbps: u32) -> u32 {
    if kbps <= 192 {
        128
    } else if kbps <= 384 {
        160
    } else {
        192
    }
}

/// Opus tiers an Opus source is rounded up to.
const OPUS_TIERS: [u32; 8] = [24, 32, 48, 64, 96, 128, 160, 192];

/// Bitrate for re-encoding the cut episode.
pub fn cut_kbps(codec: &str, source_kbps: Option<u32>) -> u32 {
    match opus_kbps(codec, source_kbps) {
        Some(k) => k,
        None => match source_kbps {
            Some(k) => *OPUS_TIERS.iter().find(|t| **t >= k).unwrap_or(&192),
            None => 96,
        },
    }
}

/// Parse ffprobe `key=value` output (stream codec_name / bit_rate and
/// format duration / bit_rate). Returns (codec, kbps, duration).
pub fn parse_probe(out: &str) -> (String, Option<u32>, Option<f64>) {
    let mut codec = String::new();
    let (mut stream_br, mut format_br, mut dur) = (None, None, None);
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
                dur = Some(d);
            }
            continue;
        };
        let num = v.trim().parse::<f64>().ok();
        match k {
            "codec_name" => codec = v.trim().to_string(),
            "duration" if in_format || dur.is_none() => dur = num.or(dur),
            "bit_rate" if in_format => format_br = num,
            "bit_rate" => stream_br = num,
            _ => {}
        }
    }
    let kbps = stream_br
        .or(format_br)
        .filter(|b| *b > 0.0)
        .map(|b| (b / 1000.0).round() as u32);
    (codec, kbps, dur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_matches_opus_convert() {
        assert_eq!(cut_kbps("mp3", Some(128)), 96);
        assert_eq!(cut_kbps("mp3", Some(192)), 128);
        assert_eq!(cut_kbps("mp3", Some(256)), 160);
        assert_eq!(cut_kbps("mp3", Some(320)), 192);
        assert_eq!(cut_kbps("mp3", None), 128);
        assert_eq!(cut_kbps("wmav2", Some(160)), 128);
        assert_eq!(cut_kbps("aac", Some(64)), 96);
        assert_eq!(cut_kbps("aac", Some(160)), 128);
        assert_eq!(cut_kbps("aac", Some(256)), 160);
        assert_eq!(cut_kbps("vorbis", Some(96)), 96);
        assert_eq!(cut_kbps("flac", Some(900)), 192);
        assert_eq!(cut_kbps("pcm_s16le", None), 192);
        assert_eq!(cut_kbps("ac3", Some(192)), 128);
        assert_eq!(cut_kbps("ac3", Some(384)), 160);
        assert_eq!(cut_kbps("eac3", Some(640)), 192);
        assert_eq!(cut_kbps("dts", Some(1509)), 192);
    }

    #[test]
    fn opus_keeps_its_tier() {
        assert_eq!(cut_kbps("opus", Some(32)), 32);
        assert_eq!(cut_kbps("opus", Some(40)), 48);
        assert_eq!(cut_kbps("opus", Some(64)), 64);
        assert_eq!(cut_kbps("opus", Some(300)), 192);
        assert_eq!(cut_kbps("opus", None), 96);
    }

    #[test]
    fn probe_output() {
        let out = "[STREAM]\ncodec_name=mp3\nbit_rate=128000\n[/STREAM]\n[FORMAT]\nduration=221.672000\nbit_rate=128115\n[/FORMAT]\n";
        assert_eq!(parse_probe(out), ("mp3".into(), Some(128), Some(221.672)));
        let out = "[STREAM]\ncodec_name=opus\nbit_rate=N/A\n[/STREAM]\n[FORMAT]\nduration=10.0\nbit_rate=48000\n[/FORMAT]\n";
        assert_eq!(parse_probe(out), ("opus".into(), Some(48), Some(10.0)));
        assert_eq!(parse_probe("100.0\n"), (String::new(), None, Some(100.0)));
    }
}
