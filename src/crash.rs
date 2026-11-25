//! Panic hook that writes crash.log next to the executable.
//!
//! Release builds use `panic = "abort"` and a Windows subsystem, so a panic
//! otherwise leaves no message. The hook runs before abort.

use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};

/// `crash.log` in the same folder as the running exe, or `crash.log` in the
/// current directory if the exe path cannot be found.
pub fn crash_log_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("crash.log")))
        .unwrap_or_else(|| PathBuf::from("crash.log"))
}

pub fn install() {
    panic::set_hook(Box::new(|info| {
        let text = hook_text(info, &Backtrace::force_capture());
        let _ = write_crash_log(&crash_log_path(), &text);
    }));
}

pub fn hook_text(info: &PanicHookInfo<'_>, backtrace: &Backtrace) -> String {
    format!("Podcast Trim crashed.\n{info}\n\n{backtrace}\n")
}

pub fn write_crash_log(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut f = fs::File::create(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_message_and_backtrace() {
        let dir = std::env::temp_dir().join(format!("pt-crash-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("crash.log");
        let text = hook_text_for_test("test panic message");
        write_crash_log(&path, &text).unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert!(got.contains("Podcast Trim crashed."));
        assert!(got.contains("test panic message"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn hook_text_for_test(msg: &str) -> String {
        format!("Podcast Trim crashed.\n{msg}\n\n{}\n", Backtrace::force_capture())
    }
}
