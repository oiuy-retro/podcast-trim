//! Small HTTP/1.1 server: one RSS feed per show and the cleaned MP3s.
//!
//! GET /feed/<show-id>.xml[?token=T]
//! GET /media/<show-id>/<episode-key>.mp3[?token=T]   (Range supported)
//!
//! When the bytes sent for a cleaned file cover the whole file, it is deleted
//! and the episode is marked Delivered (record kept).

use crate::pipeline;
use crate::rss;
use crate::store::{save_json, Library, Paths, Settings, State};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Shared {
    pub lib: Library,
    pub settings: Settings,
    pub paths: Paths,
    pub coverage: HashMap<String, Coverage>,
    pub log: Vec<String>,
}

impl Shared {
    pub fn note(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }
    pub fn save(&self) {
        let _ = save_json(&self.paths.library(), &self.lib);
    }
}

pub type SharedRef = Arc<Mutex<Shared>>;
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Byte ranges of one file that have been sent in full to some client.
#[derive(Default, Debug, Clone)]
pub struct Coverage {
    spans: Vec<(u64, u64)>, // half-open, merged, sorted
}

impl Coverage {
    pub fn add(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        self.spans.push((start, end));
        self.spans.sort();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (a, b) in self.spans.drain(..) {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.spans = merged;
    }
    pub fn is_full(&self, len: u64) -> bool {
        len > 0 && matches!(self.spans.first(), Some(&(0, e)) if e >= len)
    }
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    host: String,
    range: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut host = String::new();
    let mut range = None;
    for _ in 0..100 {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "range" => range = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Some(Request {
        method,
        path: path.to_string(),
        query,
        host,
        range,
    })
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8], head_only: bool) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if !head_only {
        let _ = stream.write_all(body);
    }
}

fn token_ok(expected: &str, given: Option<&String>) -> bool {
    let given = given.map(String::as_str).unwrap_or("");
    expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Parse `bytes=a-b`, `bytes=a-`, `bytes=-n` against `len`.
pub fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() {
            len
        } else {
            b.parse::<u64>().ok()?.saturating_add(1).min(len)
        };
        (s, e)
    };
    (start < end).then_some((start, end))
}

pub fn feed_xml(shared: &Shared, show_id: u32, host: &str) -> Option<String> {
    let show = shared.lib.show(show_id)?;
    let tok = if shared.settings.require_token {
        format!("?token={}", show.token)
    } else {
        String::new()
    };
    let base = format!("http://{host}");
    let items: Vec<rss::OutItem> = show
        .episodes
        .iter()
        .filter(|e| e.state == State::Ready)
        .map(|e| rss::OutItem {
            guid: format!("podcast-trim:{}", e.guid),
            title: e.title.clone(),
            pub_date: e.pub_date.clone(),
            url: format!("{base}/media/{show_id}/{}.mp3{tok}", e.key()),
            length: e.cleaned_bytes,
            description: format!(
                "Removed {} of ads in {} cut(s).",
                crate::cutlist::format_time(crate::cutlist::removed_seconds(&e.record.cuts)),
                e.record.cuts.len()
            ),
        })
        .collect();
    Some(rss::generate(
        &show.title,
        &format!("{base}/feed/{show_id}.xml{tok}"),
        &items,
    ))
}

fn handle(mut stream: TcpStream, shared: SharedRef, notify: Notify) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    let head = req.method == "HEAD";
    if req.method != "GET" && !head {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only",
            false,
        );
    }
    let segs: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    match segs.as_slice() {
        ["feed", file] => {
            let Some(id) = file
                .strip_suffix(".xml")
                .and_then(|i| i.parse::<u32>().ok())
            else {
                return respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain",
                    b"not found",
                    head,
                );
            };
            let body = {
                let sh = shared.lock().unwrap();
                match sh.lib.show(id) {
                    Some(show)
                        if sh.settings.require_token
                            && !token_ok(&show.token, req.query.get("token")) =>
                    {
                        None
                    }
                    Some(_) => feed_xml(&sh, id, &req.host),
                    None => None,
                }
            };
            match body {
                Some(x) => respond(
                    &mut stream,
                    "200 OK",
                    "application/rss+xml; charset=utf-8",
                    x.as_bytes(),
                    head,
                ),
                None => respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain",
                    b"not found",
                    head,
                ),
            }
        }
        ["media", show, file] => {
            let (Ok(id), Some(key)) = (show.parse::<u32>(), file.strip_suffix(".mp3")) else {
                return respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain",
                    b"not found",
                    head,
                );
            };
            let path = {
                let sh = shared.lock().unwrap();
                let show = sh.lib.show(id);
                let ok = show
                    .map(|s| {
                        !sh.settings.require_token || token_ok(&s.token, req.query.get("token"))
                    })
                    .unwrap_or(false);
                let ep = show.and_then(|s| s.episodes.iter().find(|e| e.key() == key));
                match (ok, ep) {
                    (true, Some(e)) if e.state == State::Ready => Some(sh.paths.cleaned(id, e)),
                    (true, Some(e)) if e.state == State::Delivered => {
                        drop(sh);
                        return respond(
                            &mut stream,
                            "410 Gone",
                            "text/plain",
                            b"already downloaded; regenerate it in Podcast Trim",
                            head,
                        );
                    }
                    _ => None,
                }
            };
            let Some(path) = path.and_then(|p| fs::File::open(&p).ok()) else {
                return respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain",
                    b"not found",
                    head,
                );
            };
            let key = key.to_string();
            serve_file(stream, path, id, &key, &req, shared, notify);
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain",
            b"not found",
            head,
        ),
    }
}

fn serve_file(
    mut stream: TcpStream,
    mut file: fs::File,
    id: u32,
    key: &str,
    req: &Request,
    shared: SharedRef,
    notify: Notify,
) {
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let head = req.method == "HEAD";
    let (start, end, status) = match req.range.as_deref() {
        Some(r) => match parse_range(r, len) {
            Some((s, e)) => (s, e, "206 Partial Content"),
            None => {
                let _ = write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }
        },
        None => (0, len, "200 OK"),
    };
    let mut hdr = format!(
        "HTTP/1.1 {status}\r\nContent-Type: audio/mpeg\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n",
        end - start
    );
    if status.starts_with("206") {
        hdr.push_str(&format!(
            "Content-Range: bytes {start}-{}/{len}\r\n",
            end - 1
        ));
    }
    hdr.push_str("\r\n");
    if stream.write_all(hdr.as_bytes()).is_err() || head {
        return;
    }
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut pos = start;
    let mut buf = vec![0u8; 64 * 1024];
    while pos < end {
        let want = ((end - pos) as usize).min(buf.len());
        let Ok(n) = file.read(&mut buf[..want]) else {
            break;
        };
        if n == 0 || stream.write_all(&buf[..n]).is_err() {
            break;
        }
        pos += n as u64;
    }
    let _ = stream.flush();
    drop(file);
    let mut sh = shared.lock().unwrap();
    let cov_key = format!("{id}/{key}");
    let cov = sh.coverage.entry(cov_key.clone()).or_default();
    cov.add(start, pos);
    if cov.is_full(len) {
        sh.coverage.remove(&cov_key);
        let paths = Paths::new(sh.paths.root.clone());
        let mut title = String::new();
        if let Some(ep) = sh
            .lib
            .show_mut(id)
            .and_then(|s| s.episodes.iter_mut().find(|e| e.key() == key))
        {
            pipeline::mark_delivered(&paths, id, ep);
            title = ep.title.clone();
        }
        sh.note(format!("Delivered and deleted: {title}"));
        sh.save();
        drop(sh);
        notify();
    }
}

/// Start listening; returns the bound address. Runs until the process exits.
pub fn start(shared: SharedRef, notify: Notify) -> Result<std::net::SocketAddr, String> {
    let (bind, port) = {
        let sh = shared.lock().unwrap();
        (sh.settings.bind.clone(), sh.settings.port)
    };
    let listener = TcpListener::bind((bind.as_str(), port))
        .map_err(|e| format!("Could not listen on {bind}:{port}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let shared = shared.clone();
            let notify = notify.clone();
            std::thread::spawn(move || handle(conn, shared, notify));
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Episode, Record};

    #[test]
    fn coverage() {
        let mut c = Coverage::default();
        c.add(0, 10);
        c.add(20, 30);
        assert!(!c.is_full(30));
        c.add(5, 25);
        assert!(c.is_full(30));
        assert!(!Coverage::default().is_full(0));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 100)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 1000)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 1000)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 1000)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
    }

    fn get(addr: std::net::SocketAddr, path: &str, extra: &str) -> (String, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}\r\n").unwrap();
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            String::from_utf8_lossy(&all[..split]).into_owned(),
            all[split + 4..].to_vec(),
        )
    }

    #[test]
    fn serves_feed_then_deletes_after_full_download() {
        let root = std::env::temp_dir().join(format!("pt-srv-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths::new(root.clone());
        let mut lib = Library::default();
        let id = lib.add_show("Show".into(), "u".into());
        let ep = Episode {
            guid: "g1".into(),
            title: "Ep".into(),
            pub_date: String::new(),
            source_url: "x".into(),
            state: State::Ready,
            record: Record {
                sha256: "h".into(),
                transcript: "t".into(),
                ..Default::default()
            },
            cleaned_bytes: 10,
            served: 0,
        };
        fs::create_dir_all(paths.show_dir(id)).unwrap();
        fs::write(paths.cleaned(id, &ep), b"0123456789").unwrap();
        let key = ep.key();
        lib.show_mut(id).unwrap().episodes.push(ep);
        let token = lib.show(id).unwrap().token.clone();
        let settings = Settings {
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        };
        let shared: SharedRef = Arc::new(Mutex::new(Shared {
            lib,
            settings,
            paths,
            coverage: HashMap::new(),
            log: vec![],
        }));
        let addr = start(shared.clone(), Arc::new(|| {})).unwrap();

        let (h, _) = get(addr, &format!("/feed/{id}.xml"), "");
        assert!(h.contains("404"), "token required");
        let (h, body) = get(addr, &format!("/feed/{id}.xml?token={token}"), "");
        assert!(h.contains("200"));
        let xml = String::from_utf8(body).unwrap();
        let media = format!("/media/{id}/{key}.mp3?token={token}");
        assert!(xml.contains(&format!("http://{addr}{}", media.replace('&', "&amp;"))));

        let (h, body) = get(addr, &media, "Range: bytes=0-4\r\n");
        assert!(h.contains("206") && h.contains("bytes 0-4/10"));
        assert_eq!(body, b"01234");
        assert_eq!(
            shared.lock().unwrap().lib.show(id).unwrap().episodes[0].state,
            State::Ready
        );
        let (_, body) = get(addr, &media, "Range: bytes=5-\r\n");
        assert_eq!(body, b"56789");
        std::thread::sleep(Duration::from_millis(100));
        {
            let sh = shared.lock().unwrap();
            let e = &sh.lib.show(id).unwrap().episodes[0];
            assert_eq!(e.state, State::Delivered);
            assert!(!sh.paths.cleaned(id, e).exists());
            assert_eq!(e.record.sha256, "h");
        }
        let (h, _) = get(addr, &media, "");
        assert!(h.contains("410"));
        let (_, body) = get(addr, &format!("/feed/{id}.xml?token={token}"), "");
        assert!(!String::from_utf8(body).unwrap().contains("<item>"));
    }
}
