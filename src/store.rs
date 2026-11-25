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
    /// Tool paths are filled in at run time from configuration.conf (see tools.rs).
    #[serde(skip)]
    pub ffmpeg: String,
    #[serde(skip)]
    pub ffprobe: String,
    #[serde(skip)]
    pub whisper_cli: String,
    /// The chosen ggml model, as named in Options (relative to the exe folder).
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
    /// Old export folder, read once and moved to configuration.conf [folders].
    #[serde(skip_serializing)]
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
    /// Already seen elsewhere: a `New` episode with this set is listed as
    /// OLD (dimmed) instead of NEW, and a `Delivered` one keeps its SENT tag
    /// but is dimmed. Set when an episode is sent. Files from before 0.1.2
    /// have no such field and read as `false`.
    #[serde(default)]
    pub seen: bool,
    /// Episode artwork URL from the source feed, passed on in the cleaned
    /// feed. Empty (and left out of library.json) when there is none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image_url: String,
}

impl Episode {
    pub fn key(&self) -> String {
        sha256_hex(self.guid.as_bytes())[..16].to_string()
    }
}

/// Hex digits of the audio hash that the AI prompt and reply carry.
pub const SHORT_HASH_LEN: usize = 12;

impl Record {
    /// First 12 hex digits of the original download's SHA-256, or None when
    /// the episode was never downloaded.
    pub fn short_hash(&self) -> Option<&str> {
        let h = self.sha256.get(..SHORT_HASH_LEN)?;
        h.bytes().all(|b| b.is_ascii_hexdigit()).then_some(h)
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
    /// The "only the newest few are NEW" rule has run for this show (when
    /// it was added, or once on upgrade for shows added before 0.1.2).
    #[serde(default)]
    pub seen_rule_done: bool,
    /// Show artwork URL from the source feed, passed on in the cleaned feed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image_url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub next_id: u32,
    pub shows: Vec<Show>,
    /// Episodes sent before 0.1.2 have been marked seen (once).
    #[serde(default)]
    pub sent_seen_done: bool,
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
            seen_rule_done: false,
            image_url: String::new(),
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
    pub fn queue(&self) -> PathBuf {
        self.root.join("queue.json")
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
    pub fn art_dir(&self, show: u32) -> PathBuf {
        self.show_dir(show).join("art")
    }
    pub fn art_manifest(&self, show: u32) -> PathBuf {
        self.art_dir(show).join("manifest.json")
    }
    pub fn art_file(&self, show: u32, name: &str) -> PathBuf {
        self.art_dir(show).join(name)
    }
}

pub fn load_json<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    load_json_report(path).0
}

/// Load JSON, or `T::default()` if the file is missing or not valid JSON.
/// An unreadable existing file is copied to `{name}.bad` (or `{name}.bad.N`)
/// so the original bytes are kept. The warning is meant for the status line.
pub fn load_json_report<T: for<'a> Deserialize<'a> + Default>(
    path: &Path,
) -> (T, Option<String>) {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return (T::default(), None),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => (v, None),
        Err(_) => {
            let bak = unused_bad_backup(path);
            let _ = fs::copy(path, &bak);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let bak_name = bak
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| bak.display().to_string());
            (
                T::default(),
                Some(format!(
                    "Could not read {name}; it was copied to {bak_name} and the app started without that file's data."
                )),
            )
        }
    }
}

fn unused_bad_backup(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let first = dir.join(format!("{name}.bad"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let cand = dir.join(format!("{name}.bad.{n}"));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(format!("{name}.bad.999"))
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
    fn short_hash() {
        let mut r = Record::default();
        assert_eq!(r.short_hash(), None);
        r.sha256 = sha256_hex(b"abc");
        assert_eq!(r.short_hash(), Some("ba7816bf8f01"));
        r.sha256 = "not-a-hash-at-all".into();
        assert_eq!(r.short_hash(), None);
        // Episodes from 0.1.1 files read with `seen` off.
        let e: Episode = serde_json::from_str(
            r#"{"guid":"g","title":"t","pub_date":"","source_url":"","state":"New"}"#,
        )
        .unwrap();
        assert!(!e.seen);
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

    #[test]
    fn load_json_backs_up_unreadable_file() {
        let dir = std::env::temp_dir().join(format!("pt-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("library.json");
        fs::write(&p, b"{not json").unwrap();
        let (lib, warn): (Library, Option<String>) = load_json_report(&p);
        assert!(lib.shows.is_empty());
        let warn = warn.expect("warning");
        assert!(warn.contains("library.json"));
        assert!(warn.contains("library.json.bad"));
        let bak = dir.join("library.json.bad");
        assert_eq!(fs::read(&bak).unwrap(), b"{not json");
        // A second failure must not overwrite the first backup.
        fs::write(&p, b"{also bad").unwrap();
        let (_, warn2) = load_json_report::<Library>(&p);
        assert!(warn2.unwrap().contains("library.json.bad.2"));
        assert_eq!(fs::read(dir.join("library.json.bad.2")).unwrap(), b"{also bad");
        assert_eq!(fs::read(&bak).unwrap(), b"{not json");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_json_missing_file_is_silent_default() {
        let p = std::env::temp_dir().join(format!(
            "pt-missing-{}-nope/library.json",
            std::process::id()
        ));
        let (lib, warn): (Library, Option<String>) = load_json_report(&p);
        assert!(lib.shows.is_empty());
        assert!(warn.is_none());
    }

    #[test]
    fn artwork_fields_are_optional() {
        // library.json from 0.1.1: no image_url anywhere.
        let old = r#"{"next_id":2,"shows":[{"id":1,"title":"S","feed_url":"u","token":"t",
            "episodes":[{"guid":"g","title":"E","pub_date":"d","source_url":"x","state":"New"}]}]}"#;
        let mut lib: Library = serde_json::from_str(old).unwrap();
        assert_eq!(lib.shows[0].image_url, "");
        assert_eq!(lib.shows[0].episodes[0].image_url, "");
        // Empty values stay out of the file, so it looks as before.
        let text = serde_json::to_string(&lib).unwrap();
        assert!(!text.contains("image_url"));
        lib.shows[0].image_url = "http://x/s.jpg".into();
        lib.shows[0].episodes[0].image_url = "http://x/e.jpg".into();
        let back: Library = serde_json::from_str(&serde_json::to_string(&lib).unwrap()).unwrap();
        assert_eq!(back.shows[0].image_url, "http://x/s.jpg");
        assert_eq!(back.shows[0].episodes[0].image_url, "http://x/e.jpg");
    }
}
