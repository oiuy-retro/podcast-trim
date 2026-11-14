//! `configuration.conf`: a plain text file next to Podcast Trim.exe that says
//! where the helper programs are and remembers the last folder each folder
//! picker used. The layout is shared with the other oiuy-retro Windows apps:
//!
//! - `[section]` headers, `key = value` lines, `#` comments, UTF-8.
//! - `[tools]`: `auto` (next to the exe, then each folder directly inside it,
//!   then a `bin` folder inside each of those, then PATH), `path` (PATH only),
//!   or a file path. Relative paths start from the exe's folder and
//!   `%VARIABLES%` are expanded. A missing key means `auto`.
//! - `[folders]`: one line per folder picker, rewritten by the app.
//! - Unknown keys and sections are ignored. When the app changes a value it
//!   rewrites only that line, so comments and hand edits stay.
//! - If the exe's folder can't be written to, the file goes to
//!   `%APPDATA%\Podcast Trim\configuration.conf` instead; if that fails too,
//!   remembered folders last for this session only.

use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "configuration.conf";

pub const TEMPLATE: &str = "\
# Podcast Trim configuration. Lines starting with # are comments.
# Edit with any text editor while Podcast Trim is closed.
#
# [tools] says where each helper program is:
#   auto = look next to Podcast Trim.exe, then in each folder directly inside
#          it, then in a bin folder inside each of those, then on PATH
#   path = only look on PATH
#   anything else = that file (or a folder that holds it). Relative paths
#          start from the folder Podcast Trim.exe is in, and %VARIABLES% such
#          as %USERPROFILE% are expanded. Examples:
#            ffmpeg = tools\\ffmpeg\\bin\\ffmpeg.exe
#            ffmpeg = D:\\Apps\\ffmpeg\\bin\\ffmpeg.exe
# whisper-cli.exe needs its DLLs (whisper.dll, ggml*.dll) in its own folder.
# whisper-model = auto lists every ggml-*.bin found the auto way, and you pick
# one in Options. A file path here uses that model only.

[tools]
ffmpeg = auto
ffprobe = auto
whisper-cli = auto
whisper-model = auto

# [folders] remembers where each folder picker was last. Podcast Trim updates
# these lines itself.

[folders]
export =
";

#[derive(Clone, Debug, Default)]
pub struct Conf {
    /// File the settings were read from and are saved to; None = memory only.
    pub path: Option<PathBuf>,
    /// Where to save if `path` can't be written.
    pub fallback: Option<PathBuf>,
    lines: Vec<String>,
}

fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn section_of(line: &str) -> Option<String> {
    let t = line.trim();
    (t.starts_with('[') && t.ends_with(']')).then(|| norm(&t[1..t.len() - 1]))
}

fn key_value(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') || t.starts_with('[') {
        return None;
    }
    let (k, v) = t.split_once('=')?;
    let mut v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v = &v[1..v.len() - 1];
    }
    Some((norm(k), v.to_string()))
}

impl Conf {
    pub fn parse(text: &str) -> Conf {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        Conf {
            lines: text.lines().map(str::to_string).collect(),
            ..Default::default()
        }
    }

    pub fn text(&self) -> String {
        let mut s = self.lines.join("\r\n");
        s.push_str("\r\n");
        s
    }

    /// Value and 1-based line number of `key` in `[section]`.
    pub fn get_line(&self, section: &str, key: &str) -> Option<(String, usize)> {
        let (section, key) = (norm(section), norm(key));
        let mut cur = String::new();
        let mut found = None;
        for (i, l) in self.lines.iter().enumerate() {
            if let Some(s) = section_of(l) {
                cur = s;
            } else if cur == section {
                if let Some((k, v)) = key_value(l) {
                    if k == key {
                        found = Some((v, i + 1)); // last one wins
                    }
                }
            }
        }
        found
    }

    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        self.get_line(section, key).map(|(v, _)| v)
    }

    /// Set a value, changing only its line (or adding one).
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let (sec, k) = (norm(section), norm(key));
        let mut cur = String::new();
        let mut hit = None;
        let mut last_in_section = None;
        for (i, l) in self.lines.iter().enumerate() {
            if let Some(s) = section_of(l) {
                cur = s;
                if cur == sec {
                    last_in_section = Some(i);
                }
            } else if cur == sec {
                if let Some((kk, _)) = key_value(l) {
                    last_in_section = Some(i);
                    if kk == k {
                        hit = Some(i);
                    }
                }
            }
        }
        if let Some(i) = hit {
            let old = &self.lines[i];
            let name = old.split_once('=').map(|(a, _)| a.trim_end()).unwrap_or(key);
            self.lines[i] = format!("{name} = {value}");
        } else if let Some(i) = last_in_section {
            self.lines.insert(i + 1, format!("{key} = {value}"));
        } else {
            if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                self.lines.push(String::new());
            }
            self.lines.push(format!("[{section}]"));
            self.lines.push(format!("{key} = {value}"));
        }
    }

    /// Read `configuration.conf` from `exe_dir`, or from `fallback_dir` if it
    /// only exists there. If neither exists, write the template next to the
    /// exe, or in `fallback_dir` when the exe's folder is read-only.
    pub fn load(exe_dir: &Path, fallback_dir: &Path) -> Conf {
        let main = exe_dir.join(FILE_NAME);
        let fb = fallback_dir.join(FILE_NAME);
        let read = |p: &Path| fs::read(p).ok().map(|b| Conf::parse(&String::from_utf8_lossy(&b)));
        if let Some(mut c) = read(&main) {
            // Folders remembered while the exe's folder was read-only.
            if let Some(f) = read(&fb) {
                c.merge_folders(&f);
            }
            c.path = Some(main);
            c.fallback = Some(fb);
            return c;
        }
        if let Some(mut c) = read(&fb) {
            c.path = Some(fb);
            return c;
        }
        let mut c = Conf::parse(TEMPLATE);
        c.path = Some(main);
        c.fallback = Some(fb);
        let _ = c.save();
        c
    }

    fn merge_folders(&mut self, other: &Conf) {
        let mut cur = String::new();
        let mut vals = vec![];
        for l in &other.lines {
            if let Some(s) = section_of(l) {
                cur = s;
            } else if cur == "folders" {
                if let Some((k, v)) = key_value(l) {
                    vals.push((k, v));
                }
            }
        }
        for (k, v) in vals {
            if !v.is_empty() {
                self.set("folders", &k, &v);
            }
        }
        // The window's place, only if it was never saved next to the exe.
        if self.get("window", "width").is_none() {
            let mut cur = String::new();
            let mut vals = vec![];
            for l in &other.lines {
                if let Some(s) = section_of(l) {
                    cur = s;
                } else if cur == "window" {
                    if let Some(kv) = key_value(l) {
                        vals.push(kv);
                    }
                }
            }
            for (k, v) in vals {
                self.set("window", &k, &v);
            }
        }
    }

    /// Save to `path`; if that fails, to `fallback`, and keep using it.
    pub fn save(&mut self) -> Result<(), String> {
        let text = self.text();
        let write = |p: &Path| -> std::io::Result<()> {
            if let Some(d) = p.parent() {
                fs::create_dir_all(d)?;
            }
            let tmp = p.with_extension("conf.tmp");
            fs::write(&tmp, text.as_bytes())?;
            fs::rename(&tmp, p).inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
            })
        };
        let first = match &self.path {
            Some(p) => write(p).map_err(|e| format!("{}: {e}", p.display())),
            None => Err("no configuration file".into()),
        };
        if first.is_ok() {
            return Ok(());
        }
        match self.fallback.take() {
            Some(fb) if Some(&fb) != self.path.as_ref() => match write(&fb) {
                Ok(()) => {
                    self.path = Some(fb);
                    Ok(())
                }
                Err(e) => {
                    self.path = None;
                    Err(format!("{}: {e}", fb.display()))
                }
            },
            _ => {
                self.path = None;
                first
            }
        }
    }

    /// Re-read the file this came from, so lines edited by hand while the
    /// app was running are kept, change it with `f`, and save.
    pub fn update_and_save(&mut self, f: impl FnOnce(&mut Conf)) -> Result<(), String> {
        if let Some(b) = self.path.as_ref().and_then(|p| fs::read(p).ok()) {
            let mut fresh = Conf::parse(&String::from_utf8_lossy(&b));
            fresh.path = self.path.take();
            fresh.fallback = self.fallback.take();
            *self = fresh;
        }
        f(self);
        self.save()
    }

    /// Remember a folder picker's folder and save.
    pub fn set_folder(&mut self, key: &str, dir: &str) -> Result<(), String> {
        self.set("folders", key, dir);
        self.save()
    }

    pub fn folder(&self, key: &str) -> String {
        self.get("folders", key).unwrap_or_default()
    }
}

/// `%NAME%` replaced by the environment variable; unknown names stay as is.
pub fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) if b > 0 => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) => {
                        out.push_str(&rest[..a]);
                        out.push_str(&v);
                    }
                    Err(_) => out.push_str(&rest[..a + b + 2]),
                }
                rest = &after[b + 1..];
            }
            _ => {
                out.push_str(&rest[..=a]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The nearest folder that still exists, walking up from `dir`.
pub fn nearest_existing(dir: &str) -> Option<PathBuf> {
    let mut p = PathBuf::from(dir.trim());
    if p.as_os_str().is_empty() {
        return None;
    }
    loop {
        if p.is_dir() {
            return Some(p);
        }
        if !p.pop() {
            return None;
        }
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| p.to_string_lossy().to_ascii_lowercase());
    v
}

/// Folders `auto` searches before PATH: the exe's folder, each folder
/// directly inside it, and a `bin` folder inside each of those.
pub fn auto_dirs(exe_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![exe_dir.to_path_buf()];
    for s in subdirs(exe_dir) {
        let bin = s.join("bin");
        out.push(s);
        if bin.is_dir() {
            out.push(bin);
        }
    }
    out
}

fn on_path(file: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(file))
        .find(|p| p.is_file())
}

/// Find `file` (e.g. `ffmpeg.exe`) the way `[tools] key` says.
pub fn find_tool(conf: &Conf, key: &str, file: &str, exe_dir: &Path) -> Result<PathBuf, String> {
    let (value, line) = conf.get_line("tools", key).unwrap_or(("auto".into(), 0));
    let shown = if line > 0 {
        format!("{FILE_NAME} line {line}, {key} = {value}")
    } else {
        format!("{FILE_NAME}, {key} = auto")
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => auto_dirs(exe_dir)
            .into_iter()
            .map(|d| d.join(file))
            .find(|p| p.is_file())
            .or_else(|| on_path(file))
            .ok_or_else(|| {
                format!("{file} isn't next to Podcast Trim.exe, in a folder inside it, or on PATH ({shown})")
            }),
        "path" => on_path(file).ok_or_else(|| format!("{file} isn't on PATH ({shown})")),
        _ => {
            let p = exe_dir.join(expand_vars(value.trim()));
            let p = if p.is_dir() { p.join(file) } else { p };
            if p.is_file() {
                Ok(p)
            } else {
                Err(format!("{} doesn't exist ({shown})", p.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pt-conf-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parse_get_set_keeps_comments() {
        let mut c = Conf::parse("# hi\n[Tools]\nFFmpeg = \"C:\\x\\ffmpeg.exe\"  \n# keep\n[other]\nffmpeg = no\n");
        assert_eq!(c.get("tools", "ffmpeg").unwrap(), "C:\\x\\ffmpeg.exe");
        assert_eq!(c.get_line("tools", "ffmpeg").unwrap().1, 3);
        assert_eq!(c.get("tools", "ffprobe"), None);
        c.set("tools", "ffmpeg", "auto");
        c.set("tools", "ffprobe", "path");
        c.set("folders", "export", "D:\\Out");
        let t = c.text();
        assert!(t.contains("# hi\r\n[Tools]\r\nFFmpeg = auto\r\nffprobe = path\r\n# keep\r\n"));
        assert!(t.ends_with("[folders]\r\nexport = D:\\Out\r\n"));
        assert_eq!(c.get("other", "ffmpeg").unwrap(), "no");
        let t2 = Conf::parse(TEMPLATE);
        assert_eq!(t2.get("tools", "whisper-cli").unwrap(), "auto");
        assert_eq!(t2.folder("export"), "");
    }

    #[test]
    fn vars() {
        std::env::set_var("PT_TEST_VAR", "X");
        assert_eq!(expand_vars("%PT_TEST_VAR%\\a%b%NOPE_PT%"), "X\\a%b%NOPE_PT%");
        assert_eq!(expand_vars("100%"), "100%");
    }

    #[test]
    fn auto_search_order_and_errors() {
        let d = tmp("find");
        fs::create_dir_all(d.join("ffmpeg-9/bin")).unwrap();
        fs::create_dir_all(d.join("whisper")).unwrap();
        fs::write(d.join("ffmpeg-9/bin/ff.exe"), b"x").unwrap();
        fs::write(d.join("whisper/w.exe"), b"x").unwrap();
        let c = Conf::parse(TEMPLATE);
        assert!(find_tool(&c, "ffmpeg", "ff.exe", &d).unwrap().ends_with("ffmpeg-9/bin/ff.exe"));
        assert!(find_tool(&c, "whisper-cli", "w.exe", &d).unwrap().ends_with("whisper/w.exe"));
        fs::write(d.join("ff.exe"), b"x").unwrap();
        assert_eq!(find_tool(&c, "ffmpeg", "ff.exe", &d).unwrap(), d.join("ff.exe"));
        let e = find_tool(&c, "ffprobe", "nothing-pt.exe", &d).unwrap_err();
        assert!(e.contains("line 19, ffprobe = auto"), "{e}");
        let mut c2 = c.clone();
        c2.set("tools", "ffmpeg", "ffmpeg-9\\bin\\ff.exe".replace('\\', std::path::MAIN_SEPARATOR_STR).as_str());
        assert!(find_tool(&c2, "ffmpeg", "ff.exe", &d).is_ok());
        c2.set("tools", "ffmpeg", "whisper"); // a folder holding the file
        assert!(find_tool(&c2, "ffmpeg", "w.exe", &d).is_ok());
        c2.set("tools", "ffmpeg", "missing.exe");
        assert!(find_tool(&c2, "ffmpeg", "ff.exe", &d).unwrap_err().contains("doesn't exist"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn load_writes_template_and_falls_back() {
        let d = tmp("load");
        let (exe, fb) = (d.join("exe"), d.join("appdata"));
        fs::create_dir_all(&exe).unwrap();
        let mut c = Conf::load(&exe, &fb);
        assert_eq!(c.path.as_deref(), Some(exe.join(FILE_NAME).as_path()));
        assert!(exe.join(FILE_NAME).is_file());
        c.set_folder("export", "E:\\Picks").unwrap();
        assert_eq!(Conf::load(&exe, &fb).folder("export"), "E:\\Picks");
        // Exe folder can't be written: save goes to the fallback.
        let mut ro = Conf::load(&exe, &fb);
        ro.path = Some(d.join("no-such-dir-file").join("x").join(FILE_NAME));
        fs::write(d.join("no-such-dir-file"), b"file, not a folder").unwrap();
        ro.set_folder("export", "F:\\New").unwrap();
        assert_eq!(ro.path.as_deref(), Some(fb.join(FILE_NAME).as_path()));
        // Next start reads tools from the exe's file, folders from the fallback.
        assert_eq!(Conf::load(&exe, &fb).folder("export"), "F:\\New");
        // The window's place saved in the fallback is used too, unless the
        // exe's own file has one.
        ro.set("window", "width", "1200");
        ro.save().unwrap();
        assert_eq!(Conf::load(&exe, &fb).get("window", "width").as_deref(), Some("1200"));
        let mut own = Conf::load(&exe, &fb);
        own.set("window", "width", "1000");
        own.save().unwrap();
        assert_eq!(Conf::load(&exe, &fb).get("window", "width").as_deref(), Some("1000"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn update_keeps_hand_edits() {
        let d = tmp("update");
        let mut c = Conf::load(&d, &d.join("fb"));
        let p = d.join(FILE_NAME);
        let edited = fs::read_to_string(&p).unwrap() + "\r\n[secret]\r\nmenu-sounds = on\r\n";
        fs::write(&p, edited).unwrap();
        c.update_and_save(|c| c.set("window", "width", "1000")).unwrap();
        let back = Conf::load(&d, &d.join("fb"));
        assert_eq!(back.get("secret", "menu-sounds").as_deref(), Some("on"));
        assert_eq!(back.get("window", "width").as_deref(), Some("1000"));
        assert_eq!(c.get("secret", "menu-sounds").as_deref(), Some("on"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn nearest() {
        let d = tmp("near");
        let gone = d.join("a").join("b");
        assert_eq!(nearest_existing(gone.to_str().unwrap()).unwrap(), d);
        assert_eq!(nearest_existing("  "), None);
        fs::remove_dir_all(d).unwrap();
    }
}
