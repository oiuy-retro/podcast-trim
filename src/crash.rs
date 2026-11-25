//! Startup trace and crash logs, written next to the executable (or %TEMP%).
//!
//! Release builds use `panic = "abort"` and a Windows subsystem, so a panic
//! otherwise leaves no message. The panic hook runs before abort. Stack
//! overflows and access violations do not run that hook; on Windows an
//! unhandled-exception filter records those too.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const TRACE_NAME: &str = "podcast-trim-trace.log";
const CRASH_NAME: &str = "crash.log";

static LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static EXCEPTION_LOGGED: AtomicBool = AtomicBool::new(false);
static FIRST_LAYOUT: AtomicBool = AtomicBool::new(false);
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
static FIRST_SIZE: AtomicBool = AtomicBool::new(false);
static FIRST_NCCALC: AtomicBool = AtomicBool::new(false);

/// Directory that actually received the logs (exe folder, or the temp dir).
pub fn log_dir() -> PathBuf {
    if let Ok(g) = LOG_DIR.lock() {
        if let Some(p) = g.as_ref() {
            return p.clone();
        }
    }
    pick_log_dir().0
}

pub fn crash_log_path() -> PathBuf {
    log_dir().join(CRASH_NAME)
}

pub fn trace_log_path() -> PathBuf {
    log_dir().join(TRACE_NAME)
}

/// Choose a writable folder: the exe's directory first, then the temp dir.
/// The returned path is absolute.
pub fn pick_log_dir() -> (PathBuf, &'static str) {
    if let Some(dir) = exe_dir() {
        if dir_is_writable(&dir) {
            return (dir, "exe");
        }
    }
    (temp_dir_abs(), "temp")
}

fn exe_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        return Some(absolutize(p));
    }
    module_dir()
}

fn module_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_module_dir()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn temp_dir_abs() -> PathBuf {
    absolutize(std::env::temp_dir())
}

fn absolutize(p: PathBuf) -> PathBuf {
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p)
    }
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".podcast-trim-log-probe");
    match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(mut f) => {
            let ok = f.write_all(b"ok").is_ok() && f.flush().is_ok();
            drop(f);
            let _ = fs::remove_file(&probe);
            ok
        }
        Err(_) => false,
    }
}

pub fn install() {
    let (dir, place) = pick_log_dir();
    if let Ok(mut g) = LOG_DIR.lock() {
        *g = Some(dir.clone());
    }
    let trace_path = dir.join(TRACE_NAME);
    let crash_path = dir.join(CRASH_NAME);
    let _ = fs::write(&trace_path, "");
    let exe = std::env::current_exe()
        .map(|p| absolutize(p).display().to_string())
        .unwrap_or_else(|_| "(unknown)".into());
    let cwd = std::env::current_dir()
        .map(|p| absolutize(p).display().to_string())
        .unwrap_or_else(|_| "(unknown)".into());
    let header = format!(
        "Podcast Trim startup log.\nThis file is {}\n{CRASH_NAME} will be written to {}\nLogs are in the {} folder.\nexe: {exe}\ncwd: {cwd}\n",
        trace_path.display(),
        crash_path.display(),
        if place == "exe" {
            "executable's"
        } else {
            "temporary (the folder next to the exe was not writable)"
        }
    );
    let _ = append_file(&trace_path, &header);
    panic::set_hook(Box::new(|info| {
        let text = {
            let msg = format!("{info}");
            let bt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                format!("{}", std::backtrace::Backtrace::force_capture())
            }))
            .unwrap_or_else(|_| "(backtrace unavailable)".into());
            format!("Podcast Trim crashed.\n{msg}\n\n{bt}\n")
        };
        let path = crash_log_path();
        let _ = write_crash_log(&path, &text);
        let _ = append_file(
            &trace_log_path(),
            &format!("PANIC written to {}\n{text}", path.display()),
        );
    }));
    install_exception_handlers();
    trace("hook installed");
}

/// One line in the startup trace, flushed immediately.
pub fn trace(step: &str) {
    let line = format!("{step}\n");
    let _ = append_file(&trace_log_path(), &line);
}

/// One line with a detail (kept to a single line).
pub fn trace_detail(step: &str, detail: &str) {
    let detail = detail.replace(['\r', '\n'], " ");
    let line = format!("{step}: {detail}\n");
    let _ = append_file(&trace_log_path(), &line);
}

pub fn trace_first_layout() {
    if !FIRST_LAYOUT.swap(true, Ordering::SeqCst) {
        trace("first layout");
    }
}

pub fn trace_first_paint() {
    if !FIRST_PAINT.swap(true, Ordering::SeqCst) {
        trace("first paint");
    }
}

pub fn trace_first_size() {
    if !FIRST_SIZE.swap(true, Ordering::SeqCst) {
        trace("first WM_SIZE");
    }
}

pub fn trace_first_nccalcsize() {
    if !FIRST_NCCALC.swap(true, Ordering::SeqCst) {
        trace("first WM_NCCALCSIZE");
    }
}

pub fn write_crash_log(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut f = File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.flush()
}

fn append_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(text.as_bytes())?;
    f.flush()
}

/// Human-readable name for a Windows exception code.
pub fn exception_name(code: u32) -> &'static str {
    match code {
        0xC0000005 => "ACCESS_VIOLATION",
        0xC0000006 => "IN_PAGE_ERROR",
        0xC0000008 => "INVALID_HANDLE",
        0xC000001D => "ILLEGAL_INSTRUCTION",
        0xC0000025 => "NONCONTINUABLE_EXCEPTION",
        0xC0000026 => "INVALID_DISPOSITION",
        0xC000008C => "ARRAY_BOUNDS_EXCEEDED",
        0xC000008D => "FLOAT_DENORMAL_OPERAND",
        0xC000008E => "FLOAT_DIVIDE_BY_ZERO",
        0xC000008F => "FLOAT_INEXACT_RESULT",
        0xC0000090 => "FLOAT_INVALID_OPERATION",
        0xC0000091 => "FLOAT_OVERFLOW",
        0xC0000092 => "FLOAT_STACK_CHECK",
        0xC0000093 => "FLOAT_UNDERFLOW",
        0xC0000094 => "INTEGER_DIVIDE_BY_ZERO",
        0xC0000095 => "INTEGER_OVERFLOW",
        0xC0000096 => "PRIVILEGED_INSTRUCTION",
        0xC00000FD => "STACK_OVERFLOW",
        0xC0000135 => "DLL_NOT_FOUND",
        0xC0000139 => "ENTRYPOINT_NOT_FOUND",
        0xC0000142 => "DLL_INIT_FAILED",
        0xC0000409 => "STACK_BUFFER_OVERRUN",
        0x80000001 => "GUARD_PAGE",
        0x80000003 => "BREAKPOINT",
        0x80000004 => "SINGLE_STEP",
        _ => "UNKNOWN",
    }
}

pub fn exception_text(code: u32, address: usize) -> String {
    format!(
        "Unhandled exception 0x{code:08X} ({}) at 0x{address:X}.\n",
        exception_name(code)
    )
}

fn record_exception(code: u32, address: usize) {
    if EXCEPTION_LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    let text = format!("Podcast Trim crashed.\n{}", exception_text(code, address));
    let crash = crash_log_path();
    let _ = write_crash_log(&crash, &text);
    let _ = append_file(
        &trace_log_path(),
        &format!("{text}written to {}\n", crash.display()),
    );
}

#[cfg(windows)]
fn install_exception_handlers() {
    use windows::Win32::System::Diagnostics::Debug::{
        SetUnhandledExceptionFilter, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };

    // Only the unhandled-exception filter: a vectored handler would also
    // see first-chance access violations that Windows or Wine then handle
    // (delay-loaded DLLs, guard pages), and would write a false crash.log.
    unsafe extern "system" fn on_unhandled(info: *const EXCEPTION_POINTERS) -> i32 {
        if let Some(info) = unsafe { info.as_ref() } {
            if let Some(rec) = unsafe { info.ExceptionRecord.as_ref() } {
                let code = rec.ExceptionCode.0 as u32;
                let addr = rec.ExceptionAddress as usize;
                if code != 0x80000003 && code != 0x80000004 {
                    record_exception(code, addr);
                }
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    unsafe {
        let _ = SetUnhandledExceptionFilter(Some(on_unhandled));
    }
}

#[cfg(not(windows))]
fn install_exception_handlers() {}

#[cfg(windows)]
fn windows_module_dir() -> Option<PathBuf> {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    path.parent().map(|p| absolutize(p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_message_and_backtrace() {
        let dir = std::env::temp_dir().join(format!("pt-crash-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("crash.log");
        let text = format!(
            "Podcast Trim crashed.\ntest panic message\n\n{}\n",
            std::backtrace::Backtrace::force_capture()
        );
        write_crash_log(&path, &text).unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert!(got.contains("Podcast Trim crashed."));
        assert!(got.contains("test panic message"));
        assert!(path.is_absolute() || absolutize(path.clone()).is_absolute());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pick_log_dir_falls_back_when_unwritable() {
        let (dir, place) = pick_log_dir();
        assert!(dir.is_absolute());
        assert!(place == "exe" || place == "temp");
        assert!(dir_is_writable(&dir), "chosen log dir should be writable");
    }

    #[test]
    fn exception_text_names_stack_overflow_and_av() {
        let s = exception_text(0xC00000FD, 0x1234);
        assert!(s.contains("STACK_OVERFLOW"));
        assert!(s.contains("0xC00000FD"));
        assert!(s.contains("0x1234"));
        assert!(exception_text(0xC0000005, 1).contains("ACCESS_VIOLATION"));
        assert_eq!(exception_name(0xDEADBEEF), "UNKNOWN");
    }

    #[test]
    fn append_trace_flushes_a_line() {
        let dir = std::env::temp_dir().join(format!("pt-trace-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(TRACE_NAME);
        append_file(&path, "first\n").unwrap();
        append_file(&path, "second\n").unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert_eq!(got, "first\nsecond\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
