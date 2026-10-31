//! HTTP(S) GET through WinHTTP: feeds into memory, episodes to a file.

use std::io::Write;
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

struct Parts {
    https: bool,
    host: String,
    port: u16,
    path: String,
}

fn split_url(url: &str) -> Option<Parts> {
    let (https, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let (hostport, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].to_string()),
        None => (rest, "/".into()),
    };
    if hostport.is_empty() {
        return None;
    }
    let (host, port) = if let Some((host, port)) = hostport.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            (
                host.to_string(),
                port.parse().unwrap_or(if https { 443 } else { 80 }),
            )
        } else {
            (hostport.to_string(), if https { 443 } else { 80 })
        }
    } else {
        (hostport.to_string(), if https { 443 } else { 80 })
    };
    Some(Parts {
        https,
        host,
        port,
        path,
    })
}

/// Fetch into memory (feeds). Capped at 32 MB.
pub fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    fetch(url, &mut |chunk| {
        body.extend_from_slice(chunk);
        if body.len() > 32 * 1024 * 1024 {
            return Err("The feed is larger than 32 MB.".into());
        }
        Ok(())
    })?;
    Ok(body)
}

/// Stream to `dest` (via `dest.part`, renamed when complete).
/// `progress(bytes so far, total if the server sent Content-Length)`
/// returns false to cancel.
pub fn download(
    url: &str,
    dest: &std::path::Path,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<(), String> {
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut got = 0u64;
    let res = fetch_sized(url, &mut |chunk, total| {
        file.write_all(chunk).map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if progress(got, total) {
            Ok(())
        } else {
            Err("Cancelled.".into())
        }
    });
    drop(file);
    match res {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn fetch(url: &str, sink: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    fetch_sized(url, &mut |chunk, _| sink(chunk))
}

type Sink<'a> = dyn FnMut(&[u8], Option<u64>) -> Result<(), String> + 'a;

fn fetch_sized(url: &str, sink: &mut Sink) -> Result<(), String> {
    let parts = split_url(url).ok_or_else(|| format!("Not an http(s) link: {url}"))?;
    unsafe { fetch_parts(&parts, sink) }
}

unsafe fn fetch_parts(parts: &Parts, sink: &mut Sink) -> Result<(), String> {
    let session = WinHttpOpen(
        w!("Podcast Trim"),
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        None,
        None,
        0,
    );
    if session.is_null() {
        return Err("WinHTTP did not open.".into());
    }
    let _session = Guard(session);
    WinHttpSetTimeouts(session, 15_000, 15_000, 30_000, 60_000).map_err(|err| err.to_string())?;
    // Podcast links often bounce through tracking redirects, sometimes https -> http.
    let always: u32 = 2; // WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS
    let _ = WinHttpSetOption(
        Some(session),
        WINHTTP_OPTION_REDIRECT_POLICY,
        Some(&always.to_le_bytes()),
    );
    let host = wide(&parts.host);
    let connect = WinHttpConnect(session, PCWSTR(host.as_ptr()), parts.port, 0);
    if connect.is_null() {
        return Err(format!("Could not connect to {}.", parts.host));
    }
    let _connect = Guard(connect);
    let path = wide(&parts.path);
    let flags = if parts.https {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let request = WinHttpOpenRequest(
        connect,
        w!("GET"),
        PCWSTR(path.as_ptr()),
        None,
        None,
        std::ptr::null(),
        flags,
    );
    if request.is_null() {
        return Err("WinHTTP did not open the request.".into());
    }
    let _request = Guard(request);
    WinHttpSendRequest(request, None, None, 0, 0, 0).map_err(|err| err.to_string())?;
    WinHttpReceiveResponse(request, std::ptr::null_mut()).map_err(|err| err.to_string())?;
    let mut status = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    WinHttpQueryHeaders(
        request,
        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        None,
        Some(std::ptr::addr_of_mut!(status).cast()),
        &mut size,
        std::ptr::null_mut(),
    )
    .map_err(|err| err.to_string())?;
    if status >= 400 {
        return Err(format!("{} returned HTTP {status}.", parts.host));
    }
    let mut total = 0u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    // WINHTTP_QUERY_FLAG_NUMBER64: absent or chunked responses just leave it unknown.
    let total = WinHttpQueryHeaders(
        request,
        WINHTTP_QUERY_CONTENT_LENGTH | 0x0800_0000,
        None,
        Some(std::ptr::addr_of_mut!(total).cast()),
        &mut size,
        std::ptr::null_mut(),
    )
    .ok()
    .map(|_| total)
    .filter(|t| *t > 0);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut available = 0u32;
        WinHttpQueryDataAvailable(request, &mut available).map_err(|err| err.to_string())?;
        if available == 0 {
            break;
        }
        let want = available.min(chunk.len() as u32);
        let mut read = 0u32;
        WinHttpReadData(request, chunk.as_mut_ptr().cast(), want, &mut read)
            .map_err(|err| err.to_string())?;
        sink(&chunk[..read as usize], total)?;
    }
    Ok(())
}

struct Guard(*mut core::ffi::c_void);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
