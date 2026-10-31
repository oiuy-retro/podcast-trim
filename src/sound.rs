//! Plays the optional menu sounds (see podcast_trim::menusounds) for every
//! button in the program, dialogs included, through one thread message hook.
//! Missing files mean silence; playback is async and never blocks.

use podcast_trim::config::Conf;
use podcast_trim::menusounds::{self, FILES};
use std::sync::OnceLock;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetClassNameW, SetWindowsHookExW, MSG, PM_REMOVE, WH_GETMESSAGE,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// rollover, click, release (empty = not found).
static SOUNDS: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
static mut HOVER: isize = 0;

pub fn install(conf: &Conf, exe_dir: &std::path::Path) {
    let Some(dir) = menusounds::locate(conf, exe_dir) else {
        return;
    };
    let load = |f: &str| std::fs::read(dir.join(f)).unwrap_or_default();
    let _ = SOUNDS.set([load(FILES[0]), load(FILES[1]), load(FILES[2])]);
    unsafe {
        let _ = SetWindowsHookExW(WH_GETMESSAGE, Some(hook), None, GetCurrentThreadId());
    }
}

fn play(i: usize) {
    if let Some(s) = SOUNDS.get() {
        if s[i].len() > 44 {
            unsafe {
                let _ = PlaySoundW(
                    PCWSTR(s[i].as_ptr() as *const u16),
                    None,
                    SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
                );
            }
        }
    }
}

unsafe fn is_button(hwnd: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = GetClassNameW(hwnd, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("button")
}

unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && wp.0 == PM_REMOVE.0 as usize && lp.0 != 0 {
        let msg = &*(lp.0 as *const MSG);
        match msg.message {
            WM_MOUSEMOVE => {
                let target = if is_button(msg.hwnd) { msg.hwnd.0 as isize } else { 0 };
                if target != HOVER {
                    HOVER = target;
                    if target != 0 {
                        play(0);
                    }
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if is_button(msg.hwnd) => play(1),
            WM_LBUTTONUP if is_button(msg.hwnd) => play(2),
            _ => {}
        }
    }
    CallNextHookEx(None, code, wp, lp)
}
