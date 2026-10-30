//! Settings, the show/episode library, and per-episode records.

use crate::cutlist::Cut;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Tool paths are filled in at run time from the exe folder (see tools.rs).
    #[serde(skip)]
    pub ffmpeg: String,
    #[serde(skip)]
    pub ffprobe: String,
    #[serde(skip)]
    pub whisper_cli: String,
    /// File name of the chosen ggml model in the exe folder.
    pub whisper_model: String,
    pub threads: u32,
    pub language: String,
    pub bind: String,
    pub port: u16,
    /// Address the phone uses to reach this PC; written into feed links.
    pub public_host: String,
    pub require_token: bool,
    /// Opus bitrate in kbit/s; 0 = Auto (picked from the source, see bitrate.rs).
    pub bitrate_k: u32,
    /// Output channels: Auto (from the source), Mono or Stereo.
    pub channels: Channels,
    /// Old "Mono" checkbox, read once and turned into `channels`.
    #[serde(skip_serializing)]
    pub mono: bool,
    pub export_dir: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channels {
    #[default]
    Auto,
    Mono,
    Stereo,
}

impl Channels {
    pub const ALL: [Channels; 3] = [Channels::Auto, Channels::Mono, Channels::Stereo];
    pub fn label(self) -> &'static str {
        match self {
            Channels::Auto => "Auto",
            Channels::Mono => "Mono",
            Channels::Stereo => "Stereo",
        }
    }
}

impl Settings {
    /// Upgrade settings written by older versions.
    pub fn migrate(mut self) -> Self {
        if self.mono {
            self.channels = Channels::Mono;
            self.mono = false;
        }
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ffmpeg: "ffmpeg.exe".into(),
            ffprobe: "ffprobe.exe".into(),
            whisper_cli: "whisper-cli.exe".into(),
            whisper_model: "ggml-base.en.bin".into(),
            threads: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(4),
            language: "en".into(),
            bind: "0.0.0.0".into(),
            port: 8737,
            public_host: String::new(),
            require_token: true,
            bitrate_k: 0,
            channels: Channels::Auto,
            mono: false,
            export_dir: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    New,
    Downloaded,
    Transcribed,
    /// Cleaned file is on disk and listed in the feed.
    Ready,
    /// Cleaned file was fully downloaded once and deleted. Record kept.
    Delivered,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// SHA-256 (hex) of the original download.
    pub sha256: String,
    pub source_bytes: u64,
    pub duration: f64,
    pub transcript: String,
    pub cuts: Vec<Cut>,
    /// The exact ffmpeg arguments used, kept for reference.
    pub ffmpeg_args: Vec<String>,
    /// Source audio codec and bitrate from ffprobe, and the Opus bitrate used.
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub source_kbps: Option<u32>,
    #[serde(default)]
    pub opus_kbps: u32,
    /// Source channel count, and whether a stereo source's two channels
    /// were found to be the same (None = not checked).
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub identical: Option<bool>,
    /// The cut file was encoded as mono.
    #[serde(default)]
    pub out_mono: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Episode {
    pub guid: String,
    pub title: String,
    pub pub_date: String,
    pub source_url: String,
    pub state: State,
    #[serde(default)]
    pub record: Record,
    #[serde(default)]
    pub cleaned_bytes: u64,
    #[serde(default)]
    pub served: u32,
}

impl Episode {
    pub fn key(&self) -> String {
        sha256_hex(self.guid.as_bytes())[..16].to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    pub id: u32,
    pub title: String,
    pub feed_url: String,
    /// Per-feed access token used in the feed and media URLs.
    pub token: String,
    pub episodes: Vec<Episode>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub next_id: u32,
    pub shows: Vec<Show>,
}

impl Library {
    pub fn show(&self, id: u32) -> Option<&Show> {
        self.shows.iter().find(|s| s.id == id)
    }
    pub fn show_mut(&mut self, id: u32) -> Option<&mut Show> {
        self.shows.iter_mut().find(|s| s.id == id)
    }
    pub fn add_show(&mut self, title: String, feed_url: String) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.shows.push(Show {
            id,
            title,
            feed_url,
            token: new_token(),
            episodes: Vec::new(),
        });
        id
    }
}

/// 128-bit random token from the OS-seeded hasher plus time; hex.
pub fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = Vec::new();
    for i in 0..4u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        bytes.extend_from_slice(&h.finish().to_le_bytes());
    }
    sha256_hex(&bytes)[..32].to_string()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), String> {
    let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((hex(&h.finalize()), total))
}

/// Where everything lives: `%AppData%\Podcast Trim` on Windows.
/// "1 episode", "2 episodes".
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// This PC's LAN IPv4 address: the source address the OS would use to reach
/// the internet. A UDP "connect" only picks a route; nothing is sent.
pub fn lan_ipv4() -> Option<std::net::Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PODCAST_TRIM_DATA") {
        return PathBuf::from(d);
    }
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Podcast Trim")
}

pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Paths { root }
    }
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }
    pub fn library(&self) -> PathBuf {
        self.root.join("library.json")
    }
    pub fn show_dir(&self, show: u32) -> PathBuf {
        self.root.join("shows").join(show.to_string())
    }
    pub fn original(&self, show: u32, ep: &Episode) -> PathBuf {
        self.show_dir(show).join(format!("{}.orig", ep.key()))
    }
    pub fn cleaned(&self, show: u32, ep: &Episode) -> PathBuf {
        self.show_dir(show).join(format!("{}.opus", ep.key()))
    }
    pub fn work(&self, show: u32, ep: &Episode, ext: &str) -> PathBuf {
        self.show_dir(show).join(format!("{}.{ext}", ep.key()))
    }
}

pub fn load_json<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write via a temp file and rename so a crash never leaves half a file.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let d = std::env::temp_dir().join(format!("pt-hash-{}", std::process::id()));
        fs::write(&d, b"abc").unwrap();
        assert_eq!(sha256_file(&d).unwrap(), (sha256_hex(b"abc"), 3));
        fs::remove_file(d).unwrap();
        assert_ne!(new_token(), new_token());
        assert_eq!(new_token().len(), 32);
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(1, "episode"), "1 episode");
        assert_eq!(plural(0, "episode"), "0 episodes");
        assert_eq!(plural(2, "new episode"), "2 new episodes");
    }

    #[test]
    fn library_roundtrip() {
        let mut lib = Library::default();
        let a = lib.add_show("A".into(), "u".into());
        let b = lib.add_show("B".into(), "v".into());
        assert_eq!((a, b), (1, 2));
        let p = std::env::temp_dir()
            .join(format!("pt-lib-{}", std::process::id()))
            .join("library.json");
        save_json(&p, &lib).unwrap();
        let back: Library = load_json(&p);
        assert_eq!(back.show(2).unwrap().title, "B");
        let s: Settings = serde_json::from_str("{\"port\":1}").unwrap();
        assert_eq!((s.port, s.bitrate_k, s.channels), (1, 0, Channels::Auto));
        let old: Settings = serde_json::from_str(r#"{"mono": true}"#).unwrap();
        assert_eq!(old.migrate().channels, Channels::Mono);
        let text = serde_json::to_string(&Settings::default()).unwrap();
        assert!(text.contains("\"channels\":\"Auto\"") && !text.contains("\"mono\""));
    }
}
