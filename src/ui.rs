//! Win32 window drawn in the style of GoldSource-era VGUI menus:
//! olive panels, light text with khaki for the active tab and checked items,
//! 1px light/dark bevels, flat beveled buttons, Tahoma 13 px without smoothing.
//! The window draws its own caption, tabs, buttons, and scroll bars.

use crate::net;
use podcast_trim::cutlist::{format_time, removed_seconds, Cut};
use podcast_trim::pipeline::{self, Env};
use podcast_trim::server::{self, Shared, SharedRef};
use podcast_trim::store::{self, load_json, save_json, Library, Paths, Settings, State};
use podcast_trim::{ffargs, prompt, rss};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, GetDC, GetDeviceCaps, InvalidateRect, LineTo, MoveToEx, RedrawWindow, ReleaseDC,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HFONT, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, RDW_ALLCHILDREN, RDW_ERASE,
    RDW_INVALIDATE, TRANSPARENT,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED};
use windows::Win32::UI::WindowsAndMessaging::*;

// GoldSource VGUI-like palette (approximate, picked by eye).
const BG: u32 = rgb(76, 88, 68);
const FIELD: u32 = rgb(62, 70, 55);
const LIGHT: u32 = rgb(136, 145, 128);
const DARK: u32 = rgb(40, 46, 34);
const TEXT: u32 = rgb(216, 222, 211);
const TAN: u32 = rgb(196, 181, 80);
const DIM: u32 = rgb(117, 128, 111);
const SEL: u32 = rgb(149, 136, 49);
/// Scroll bar track and read-only ("disabled") field text, measured from the HLDS dialogs.
const TRACK: u32 = rgb(90, 106, 79);
const DISABLED_TEXT: u32 = rgb(149, 161, 141);
const BLACK: u32 = rgb(0, 0, 0);

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const WM_REFRESH: u32 = WM_APP + 1;
/// A worker reported progress; repaint the bar and the status line.
const WM_PROGRESS: u32 = WM_APP + 2;
/// The model drop-down list was closed on item `wparam`.
const WM_MODEL_PICK: u32 = WM_APP + 3;
const DROP_ROW: i32 = 18;

// Control ids
const ID_SHOWS: i32 = 100;
const ID_EPS: i32 = 101;
const ID_URL: i32 = 102;
const ID_VIEW: i32 = 103;
const ID_REPLY: i32 = 104;
const ID_FEED: i32 = 105;
const ID_STATUS: i32 = 106;
const B_ADD: i32 = 200;
const B_REFRESH: i32 = 201;
const B_PROCESS: i32 = 202;
const B_PROMPT: i32 = 203;
const B_CHECK: i32 = 204;
const B_APPROVE: i32 = 205;
const B_REGEN: i32 = 206;
const B_EXPORT: i32 = 207;
const B_COPYFEED: i32 = 208;
const B_TAB_MAIN: i32 = 209;
const B_SAVE: i32 = 210;
const B_TAB_OPT: i32 = 211;
const B_MIN: i32 = 215;
const B_CLOSE: i32 = 216;
const B_CANCEL: i32 = 217;
const ID_PROGRESS: i32 = 107;
const B_TOKEN: i32 = 212;
const OPT_BASE: i32 = 300;

const B_CHANNELS: i32 = 213;
const B_MODEL: i32 = 214;

/// Edit rows on the Options page; row 0 is the model picker button.
const OPTION_LABELS: [&str; 7] = [
    "CPU threads",
    "Language",
    "Listen address",
    "Port",
    "PC address for phone",
    "Opus bitrate (kbps)",
    "Export folder",
];

struct Ui {
    hwnd: HWND,
    shared: SharedRef,
    font: HFONT,
    brush_field: HBRUSH,
    brush_bg: HBRUSH,
    dpi: i32,
    ctl: HashMap<i32, HWND>,
    show_ids: Vec<u32>,
    ep_guids: Vec<String>,
    /// Cuts that passed validation, waiting for approval: (show, guid, cuts).
    pending: Option<(u32, String, Vec<Cut>)>,
    busy: bool,
    options: bool,
    token_on: bool,
    channels: store::Channels,
    models: Vec<String>,
    model: String,
    server_addr: String,
    /// Bevelled frames: (rect, page, raised).
    frames: Vec<(RECT, u8, bool)>,
    labels: Vec<(RECT, String, u8, LabelKind)>,
    /// Every top-level child and the page it belongs to.
    pages: Vec<(HWND, u8)>,
    icon: HICON,
    /// How the next created element follows a resize (see `Anchor`).
    anchor: Anchor,
    items: Vec<Placed>,
    feed_frame: usize,
    /// Companion tools missing from the exe folder, as a full message.
    missing: String,
    /// Detected LAN address, used when "PC address for phone" is blank.
    lan: Option<std::net::Ipv4Addr>,
}

/// How an element follows the window size: fractions of the extra width
/// added to its left and right edge, and of the extra height added to its top
/// and bottom edge.
#[derive(Clone, Copy)]
struct Anchor(f32, f32, f32, f32);

const FIXED: Anchor = Anchor(0.0, 0.0, 0.0, 0.0);

/// An element, its base rect, and its anchor.
type Placed = (Item, (i32, i32, i32, i32), Anchor);

/// Something `layout` moves: base rect (96-DPI pixels at 900x600) and anchor.
enum Item {
    Win(HWND),
    Frame(usize),
    Label(usize),
    Boxed {
        clip: HWND,
        ctl: HWND,
        sb: HWND,
        frame: usize,
        header: Option<usize>,
        pad: i32,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum LabelKind {
    Plain,
    Header,
}

static BRUSH_BG: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static BRUSH_FIELD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static STATUS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static FONT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn font() -> HFONT {
    HFONT(FONT.load(std::sync::atomic::Ordering::Relaxed) as _)
}

static DPI: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(96);
static FEED: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static MAIN: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
/// Progress of the running job in 1/1000, and what it is doing.
static PROGRESS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static STAGE: Mutex<String> = Mutex::new(String::new());
static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The external program the worker is waiting on, so Cancel can stop it.
static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

const CANCELLED: &str = "Cancelled.";

fn cancelled() -> bool {
    CANCEL.load(std::sync::atomic::Ordering::SeqCst)
}

/// Stop the running job: set the flag (downloads check it per chunk) and
/// kill the child process, if any. The worker sees its pipes close and returns.
fn cancel_job() {
    CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(c) = CHILD.lock().unwrap().as_mut() {
        let _ = c.kill();
    }
}

fn set_progress(stage: &str, fraction: f64) {
    let v = (fraction.clamp(0.0, 1.0) * 1000.0).round() as i32;
    let old = PROGRESS.swap(v, std::sync::atomic::Ordering::Relaxed);
    let changed_stage = {
        let mut st = STAGE.lock().unwrap();
        if *st != stage {
            *st = stage.to_string();
            true
        } else {
            false
        }
    };
    if old != v || changed_stage {
        unsafe {
            let h = HWND(MAIN.load(std::sync::atomic::Ordering::Relaxed) as _);
            let _ = PostMessageW(h, WM_PROGRESS, WPARAM(0), LPARAM(0));
        }
    }
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    // try_borrow: Win32 re-enters wndproc while a handler holds the state.
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

fn set_text(h: HWND, s: &str) {
    let crlf = s.replace("\r\n", "\n").replace('\n', "\r\n");
    let w = wide(&crlf);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(w.as_ptr()));
    }
}

fn copy_to_clipboard(hwnd: HWND, s: &str) -> bool {
    let w = wide(&s.replace("\r\n", "\n").replace('\n', "\r\n"));
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) else {
            let _ = CloseClipboard();
            return false;
        };
        let p = GlobalLock(mem) as *mut u16;
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        let _ = GlobalUnlock(mem);
        let ok = SetClipboardData(13, HANDLE(mem.0)).is_ok(); // CF_UNICODETEXT
        let _ = CloseClipboard();
        ok
    }
}

/// Real process and network access.
struct SysEnv;

impl Env for SysEnv {
    fn fetch(&self, url: &str, dest: &Path) -> Result<(), String> {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        set_progress("Downloading", 0.0);
        net::download(url, dest, &mut |got, total| {
            match total {
                Some(t) => set_progress("Downloading", got as f64 / t as f64),
                None => set_progress(&format!("Downloading ({} MB)", got >> 20), 0.0),
            }
            !cancelled()
        })
    }
    fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        self.run_watch(program, args, &mut |_| {})
    }
    fn run_watch(
        &self,
        program: &str,
        args: &[String],
        line: &mut dyn FnMut(&str),
    ) -> Result<String, String> {
        use std::io::BufRead;
        use std::process::Stdio;
        if cancelled() {
            return Err(CANCELLED.into());
        }
        let mut child = std::process::Command::new(program)
            .args(args)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start {program}: {e}. Check the path in Options."))?;
        let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
        let pipes: [(bool, Box<dyn std::io::Read + Send>); 2] = [
            (true, Box::new(child.stdout.take().unwrap())),
            (false, Box::new(child.stderr.take().unwrap())),
        ];
        let readers: Vec<_> = pipes
            .into_iter()
            .map(|(is_out, pipe)| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for l in std::io::BufReader::new(pipe).split(b'\n') {
                        let Ok(l) = l else { break };
                        let l = String::from_utf8_lossy(&l)
                            .trim_end_matches('\r')
                            .to_string();
                        if tx.send((is_out, l)).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        drop(tx);
        *CHILD.lock().unwrap() = Some(child);
        // Cancel may have landed between the check above and storing the child.
        if cancelled() {
            if let Some(c) = CHILD.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
        }
        let mut stdout = String::new();
        let mut tail: std::collections::VecDeque<String> = Default::default();
        for (is_out, l) in rx {
            line(&l);
            if is_out {
                stdout.push_str(&l);
                stdout.push('\n');
            } else if !l.trim().is_empty() && whisper_noise(&l).is_none() {
                tail.push_back(l);
                if tail.len() > 4 {
                    tail.pop_front();
                }
            }
        }
        for r in readers {
            let _ = r.join();
        }
        let status = CHILD.lock().unwrap().take().map(|mut c| c.wait());
        if cancelled() {
            return Err(CANCELLED.into());
        }
        match status {
            Some(Ok(st)) if st.success() => Ok(stdout),
            Some(Ok(_)) => Err(format!(
                "{program} failed: {}",
                tail.into_iter().collect::<Vec<_>>().join(" | ")
            )),
            Some(Err(e)) => Err(format!("{program}: {e}")),
            None => Err(CANCELLED.into()),
        }
    }
    fn progress(&self, stage: &str, fraction: f64) {
        set_progress(stage, fraction);
    }
}

/// whisper.cpp progress lines are not useful in an error message.
fn whisper_noise(l: &str) -> Option<f64> {
    podcast_trim::progress::whisper_percent(l)
}

fn scale(v: i32, dpi: i32) -> i32 {
    v * dpi / 96
}

/// Tahoma, 13 px, no smoothing: what the GoldSource VGUI dialogs use.
fn make_font(px: i32, bold: bool) -> HFONT {
    unsafe {
        CreateFontW(
            px.neg(),
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            0,
            0,
            0,
            1,
            0,
            0,
            3,
            0,
            w!("Tahoma"),
        )
    }
}

trait Neg {
    fn neg(self) -> i32;
}
impl Neg for i32 {
    fn neg(self) -> i32 {
        -self
    }
}

pub fn run() -> i32 {
    let paths = Paths::new(store::data_dir());
    let _ = std::fs::create_dir_all(&paths.root);
    let settings: Settings = if paths.settings().exists() {
        load_json::<Settings>(&paths.settings()).migrate()
    } else {
        Settings::default()
    };
    let mut lib: Library = load_json(&paths.library());
    // Clean up anything left behind by a delivery that couldn't delete at the time.
    for show in &lib.shows {
        for ep in &show.episodes {
            if ep.state == State::Delivered {
                let _ = std::fs::remove_file(paths.cleaned(show.id, ep));
            }
        }
    }
    lib.next_id = lib
        .next_id
        .max(lib.shows.iter().map(|s| s.id + 1).max().unwrap_or(1));
    let token_on = settings.require_token;
    let channels = settings.channels;
    let model = settings.whisper_model.clone();
    let shared: SharedRef = Arc::new(Mutex::new(Shared {
        lib,
        settings,
        paths,
        coverage: HashMap::new(),
        log: Vec::new(),
    }));

    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("PodcastTrimWindow"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(instance, PCWSTR(1 as _)).unwrap_or_default(),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&class);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(clip_proc),
            hInstance: instance,
            lpszClassName: w!("PodcastTrimClip"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(scroll_proc),
            hInstance: instance,
            lpszClassName: w!("PodcastTrimScroll"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(drop_proc),
            hInstance: instance,
            lpszClassName: w!("PodcastTrimDrop"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            style: CS_DROPSHADOW,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(progress_proc),
            hInstance: instance,
            lpszClassName: w!("PodcastTrimProgress"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        });
        let dc = GetDC(None);
        let dpi = GetDeviceCaps(dc, LOGPIXELSX).max(96);
        ReleaseDC(None, dc);
        DPI.store(dpi, std::sync::atomic::Ordering::Relaxed);
        let (w, h) = (scale(WIN_W, dpi), scale(WIN_H, dpi));
        // Borderless: the app draws its own VGUI-style caption and frame.
        let style = WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let r = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let (sx, sy) = (
            (GetSystemMetrics(SM_CXSCREEN) - w) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - h) / 2,
        );
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PodcastTrimWindow"),
            w!("Podcast Trim"),
            style,
            sx.max(0),
            sy.max(0),
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            instance,
            None,
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            return 1;
        }
        MAIN.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
        let ui = Ui {
            hwnd,
            shared: shared.clone(),
            font: make_font(scale(13, dpi), false),
            brush_field: CreateSolidBrush(COLORREF(FIELD)),
            brush_bg: CreateSolidBrush(COLORREF(BG)),
            dpi,
            ctl: HashMap::new(),
            show_ids: vec![],
            ep_guids: vec![],
            pending: None,
            busy: false,
            options: false,
            token_on,
            channels,
            models: podcast_trim::tools::models(&podcast_trim::tools::exe_dir()),
            model,
            server_addr: String::new(),
            frames: vec![],
            labels: vec![],
            pages: vec![],
            anchor: FIXED,
            items: vec![],
            feed_frame: usize::MAX,
            missing: String::new(),
            lan: store::lan_ipv4(),
            icon: LoadImageW(
                instance,
                PCWSTR(1 as _),
                IMAGE_ICON,
                16,
                16,
                LR_DEFAULTCOLOR,
            )
            .map(|h| HICON(h.0))
            .unwrap_or_default(),
        };
        BRUSH_BG.store(ui.brush_bg.0 as isize, std::sync::atomic::Ordering::Relaxed);
        BRUSH_FIELD.store(
            ui.brush_field.0 as isize,
            std::sync::atomic::Ordering::Relaxed,
        );
        FONT.store(ui.font.0 as isize, std::sync::atomic::Ordering::Relaxed);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        build_controls(instance);
        layout();
        let hraw = hwnd.0 as isize;
        let notify: server::Notify = Arc::new(move || {
            let _ = PostMessageW(HWND(hraw as _), WM_REFRESH, WPARAM(0), LPARAM(0));
        });
        let started = server::start(shared.clone(), notify);
        with(|ui| match started {
            Ok(addr) => {
                ui.server_addr = addr.to_string();
                let dir = podcast_trim::tools::exe_dir();
                let miss = podcast_trim::tools::missing(&dir);
                if miss.is_empty() {
                    status(ui, &format!("Feed server listening on {addr}."));
                } else {
                    ui.missing = podcast_trim::tools::missing_message(&dir, &miss);
                    status(
                        ui,
                        &format!(
                            "{} missing next to Podcast Trim.exe. The list is in Details.",
                            store::plural(miss.len(), "file")
                        ),
                    );
                }
            }
            Err(e) => status(ui, &e),
        });
        refresh_lists();
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    0
}

fn status(ui: &Ui, msg: &str) {
    if let Some(h) = ui.ctl.get(&ID_STATUS) {
        set_text(*h, msg);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn child(
    ui: &mut Ui,
    instance: HINSTANCE,
    parent: HWND,
    class: PCWSTR,
    text: &str,
    style: u32,
    id: i32,
    page: u8,
    r: (i32, i32, i32, i32),
) -> HWND {
    let d = ui.dpi;
    let t = wide(text);
    let h = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR(t.as_ptr()),
        WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
        scale(r.0, d),
        scale(r.1, d),
        scale(r.2, d),
        scale(r.3, d),
        parent,
        HMENU(id as _),
        instance,
        None,
    )
    .unwrap_or_default();
    SendMessageW(h, WM_SETFONT, WPARAM(ui.font.0 as usize), LPARAM(1));
    ui.ctl.insert(id, h);
    ui.pages.push((h, page));
    if parent == ui.hwnd {
        ui.items.push((Item::Win(h), r, ui.anchor));
    }
    h
}

const BTN: u32 = BS_OWNERDRAW as u32 | WS_TABSTOP.0;
const EDIT: u32 = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
const MEMO: u32 = ES_MULTILINE as u32
    | ES_AUTOVSCROLL as u32
    | ES_WANTRETURN as u32
    | WS_VSCROLL.0
    | WS_TABSTOP.0;
const LIST: u32 = LBS_NOTIFY as u32
    | LBS_OWNERDRAWFIXED as u32
    | LBS_HASSTRINGS as u32
    | LBS_NOINTEGRALHEIGHT as u32
    | LBS_DISABLENOSCROLL as u32
    | WS_VSCROLL.0
    | WS_TABSTOP.0;

const PAGE_ALL: u8 = 0;
const PAGE_MAIN: u8 = 1;
const PAGE_OPT: u8 = 2;

/// Window size in 96-DPI pixels.
const WIN_W: i32 = 900;
const WIN_H: i32 = 600;
const CAPTION_H: i32 = 26;
const SB_W: i32 = 17;

fn sr(ui: &Ui, x: i32, y: i32, w: i32, h: i32) -> RECT {
    let d = ui.dpi;
    RECT {
        left: scale(x, d),
        top: scale(y, d),
        right: scale(x + w, d),
        bottom: scale(y + h, d),
    }
}

/// A plain sunken field (single-line edit) with its frame.
fn field(
    ui: &mut Ui,
    instance: HINSTANCE,
    style: u32,
    id: i32,
    page: u8,
    r: (i32, i32, i32, i32),
) -> HWND {
    let fr = sr(ui, r.0, r.1, r.2, r.3);
    ui.frames.push((fr, page, false));
    ui.items
        .push((Item::Frame(ui.frames.len() - 1), r, ui.anchor));
    unsafe {
        child(
            ui,
            instance,
            ui.hwnd,
            w!("EDIT"),
            "",
            style,
            id,
            page,
            (r.0 + 4, r.1 + 3, r.2 - 6, r.3 - 5),
        )
    }
}

/// A sunken box holding a list or multi-line edit, an optional raised column
/// header, and a themed scroll bar. The control's own scroll bar is pushed
/// outside a clipping parent so only ours shows.
#[allow(clippy::too_many_arguments)]
fn boxed(
    ui: &mut Ui,
    instance: HINSTANCE,
    class: PCWSTR,
    style: u32,
    id: i32,
    page: u8,
    header: Option<&str>,
    r: (i32, i32, i32, i32),
) {
    let d = ui.dpi;
    let fr = sr(ui, r.0, r.1, r.2, r.3);
    ui.frames.push((fr, page, false));
    let frame = ui.frames.len() - 1;
    let (ix, iy, iw, ih) = (r.0 + 1, r.1 + 1, r.2 - 2, r.3 - 2);
    let hh = if header.is_some() { 20 } else { 0 };
    let header_at = header.map(|_| ui.labels.len());
    if let Some(h) = header {
        ui.labels.push((
            sr(ui, ix, iy, iw - SB_W, hh),
            h.to_string(),
            page,
            LabelKind::Header,
        ));
    }
    let pad = if class == w!("EDIT") { 3 } else { 0 };
    unsafe {
        let clip = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("PodcastTrimClip"),
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            scale(ix, d),
            scale(iy + hh, d),
            scale(iw - SB_W, d),
            scale(ih - hh, d),
            ui.hwnd,
            None,
            instance,
            None,
        )
        .unwrap_or_default();
        ui.pages.push((clip, page));
        let t = wide("");
        let ctl = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
            scale(pad, d),
            scale(pad, d),
            scale(iw - SB_W - pad, d) + GetSystemMetrics(SM_CXVSCROLL),
            scale(ih - hh - pad, d),
            clip,
            HMENU(id as _),
            instance,
            None,
        )
        .unwrap_or_default();
        SendMessageW(ctl, WM_SETFONT, WPARAM(ui.font.0 as usize), LPARAM(1));
        ui.ctl.insert(id, ctl);
        let sb = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PodcastTrimScroll"),
            None,
            WS_CHILD | WS_VISIBLE,
            scale(ix + iw - SB_W, d),
            scale(iy, d),
            scale(SB_W, d),
            scale(ih, d),
            ui.hwnd,
            None,
            instance,
            None,
        )
        .unwrap_or_default();
        SetWindowLongPtrW(sb, GWLP_USERDATA, ctl.0 as isize);
        SetTimer(sb, 1, 120, None);
        ui.pages.push((sb, page));
        let item = Item::Boxed {
            clip,
            ctl,
            sb,
            frame,
            header: header_at,
            pad,
        };
        ui.items.push((item, r, ui.anchor));
    }
}

/// Base rect moved by its anchor for a window `dx`/`dy` larger than 900x600.
fn anchored(r: (i32, i32, i32, i32), a: Anchor, dx: i32, dy: i32) -> (i32, i32, i32, i32) {
    let f = |v: f32, d: i32| (v * d as f32).round() as i32;
    let x0 = r.0 + f(a.0, dx);
    let x1 = r.0 + r.2 + f(a.1, dx);
    let y0 = r.1 + f(a.2, dy);
    let y1 = r.1 + r.3 + f(a.3, dy);
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

/// Place everything for the current client size.
fn layout() {
    with(|ui| unsafe {
        let d = ui.dpi;
        let mut rc = RECT::default();
        let _ = GetClientRect(ui.hwnd, &mut rc);
        let (w, h) = (rc.right * 96 / d, rc.bottom * 96 / d);
        let (dx, dy) = ((w - WIN_W).max(0), (h - WIN_H).max(0));
        let place = |h: HWND, x: i32, y: i32, cw: i32, ch: i32| {
            let _ = SetWindowPos(
                h,
                None,
                scale(x, d),
                scale(y, d),
                scale(cw, d),
                scale(ch, d),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        };
        for i in 0..ui.items.len() {
            let (x, y, cw, ch) = anchored(ui.items[i].1, ui.items[i].2, dx, dy);
            match ui.items[i].0 {
                Item::Win(h) => place(h, x, y, cw, ch),
                Item::Frame(f) => ui.frames[f].0 = sr(ui, x, y, cw, ch),
                Item::Label(l) => ui.labels[l].0 = sr(ui, x, y, cw, ch),
                Item::Boxed {
                    clip,
                    ctl,
                    sb,
                    frame,
                    header,
                    pad,
                } => {
                    ui.frames[frame].0 = sr(ui, x, y, cw, ch);
                    let (ix, iy, iw, ih) = (x + 1, y + 1, cw - 2, ch - 2);
                    let hh = if header.is_some() { 20 } else { 0 };
                    if let Some(l) = header {
                        ui.labels[l].0 = sr(ui, ix, iy, iw - SB_W, hh);
                    }
                    place(clip, ix, iy + hh, iw - SB_W, ih - hh);
                    let _ = SetWindowPos(
                        ctl,
                        None,
                        scale(pad, d),
                        scale(pad, d),
                        scale(iw - SB_W - pad, d) + GetSystemMetrics(SM_CXVSCROLL),
                        scale(ih - hh - pad, d),
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    place(sb, ix + iw - SB_W, iy, SB_W, ih);
                }
            }
        }
        let _ = RedrawWindow(
            ui.hwnd,
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn button(ui: &mut Ui, instance: HINSTANCE, text: &str, id: i32, page: u8, x: i32, y: i32, w: i32) {
    unsafe {
        child(
            ui,
            instance,
            ui.hwnd,
            w!("BUTTON"),
            text,
            BTN,
            id,
            page,
            (x, y, w, 24),
        );
    }
}

fn label(ui: &mut Ui, text: &str, page: u8, x: i32, y: i32, w: i32) {
    let r = sr(ui, x, y, w, 16);
    ui.labels.push((r, text.into(), page, LabelKind::Plain));
    ui.items
        .push((Item::Label(ui.labels.len() - 1), (x, y, w, 16), ui.anchor));
}

fn build_controls(instance: HINSTANCE) {
    with(|ui| unsafe {
        let (l, e) = (w!("LISTBOX"), w!("EDIT"));
        let bt = w!("BUTTON");
        // Caption buttons and tabs
        ui.anchor = Anchor(1.0, 1.0, 0.0, 0.0);
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "",
            BS_OWNERDRAW as u32,
            B_MIN,
            PAGE_ALL,
            (WIN_W - 48, 5, 18, 17),
        );
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "",
            BS_OWNERDRAW as u32,
            B_CLOSE,
            PAGE_ALL,
            (WIN_W - 26, 5, 18, 17),
        );
        ui.anchor = FIXED;
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "Episodes",
            BS_OWNERDRAW as u32,
            B_TAB_MAIN,
            PAGE_ALL,
            (14, 32, 80, 21),
        );
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "Options",
            BS_OWNERDRAW as u32,
            B_TAB_OPT,
            PAGE_ALL,
            (96, 32, 76, 21),
        );

        // Episodes page
        let m = PAGE_MAIN;
        ui.anchor = Anchor(0.0, 0.3, 0.0, 1.0);
        boxed(
            ui,
            instance,
            l,
            LIST,
            ID_SHOWS,
            m,
            Some("Show"),
            (24, 68, 264, 296),
        );
        ui.anchor = Anchor(0.0, 0.3, 1.0, 1.0);
        label(ui, "Show RSS link", m, 24, 374, 264);
        field(ui, instance, EDIT, ID_URL, m, (24, 392, 264, 22));
        ui.anchor = Anchor(0.0, 0.0, 1.0, 1.0);
        button(ui, instance, "Add show", B_ADD, m, 24, 422, 90);
        button(ui, instance, "Check for new", B_REFRESH, m, 120, 422, 110);

        ui.anchor = Anchor(0.3, 0.65, 0.0, 1.0);
        boxed(
            ui,
            instance,
            l,
            LIST,
            ID_EPS,
            m,
            Some("Episode"),
            (300, 68, 296, 296),
        );
        ui.anchor = Anchor(0.3, 0.3, 1.0, 1.0);
        button(
            ui,
            instance,
            "Download + transcribe",
            B_PROCESS,
            m,
            300,
            374,
            150,
        );
        button(ui, instance, "Copy AI prompt", B_PROMPT, m, 456, 374, 120);
        button(ui, instance, "Regenerate", B_REGEN, m, 300, 404, 100);
        button(ui, instance, "Export Opus...", B_EXPORT, m, 406, 404, 110);

        ui.anchor = Anchor(0.65, 1.0, 0.0, 1.0);
        boxed(
            ui,
            instance,
            e,
            MEMO | ES_READONLY as u32,
            ID_VIEW,
            m,
            Some("Details"),
            (608, 68, 268, 196),
        );
        ui.anchor = Anchor(0.65, 1.0, 1.0, 1.0);
        label(ui, "Paste the AI's reply", m, 608, 274, 268);
        boxed(
            ui,
            instance,
            e,
            MEMO,
            ID_REPLY,
            m,
            None,
            (608, 292, 268, 72),
        );
        ui.anchor = Anchor(0.65, 0.65, 1.0, 1.0);
        button(ui, instance, "Check cuts", B_CHECK, m, 608, 374, 96);
        button(ui, instance, "Approve and cut", B_APPROVE, m, 710, 374, 120);

        ui.anchor = Anchor(0.0, 0.0, 1.0, 1.0);
        label(ui, "Feed link for your podcast app", m, 24, 458, 400);
        ui.anchor = Anchor(0.0, 1.0, 1.0, 1.0);
        // Read-only, drawn like the HLDS dialogs' disabled fields.
        field(
            ui,
            instance,
            EDIT | ES_READONLY as u32,
            ID_FEED,
            m,
            (24, 476, 730, 22),
        );
        ui.feed_frame = ui.frames.len() - 1;
        FEED.store(
            ui.ctl[&ID_FEED].0 as isize,
            std::sync::atomic::Ordering::Relaxed,
        );
        ui.anchor = Anchor(1.0, 1.0, 1.0, 1.0);
        button(ui, instance, "Copy feed link", B_COPYFEED, m, 766, 475, 110);
        // Status line, then the job's progress bar and Cancel, laid out like
        // the GoldSource "Loading..." dialog.
        ui.anchor = Anchor(0.0, 1.0, 1.0, 1.0);
        child(
            ui,
            instance,
            ui.hwnd,
            w!("STATIC"),
            "",
            0x0C | 0x80 | 0x4000, // SS_LEFTNOWORDWRAP | SS_NOPREFIX | SS_ENDELLIPSIS
            ID_STATUS,
            PAGE_ALL,
            (24, 516, 852, 16),
        );
        child(
            ui,
            instance,
            ui.hwnd,
            w!("PodcastTrimProgress"),
            "",
            0,
            ID_PROGRESS,
            PAGE_ALL,
            (24, 539, 730, 24),
        );
        ui.anchor = Anchor(1.0, 1.0, 1.0, 1.0);
        button(ui, instance, "Cancel", B_CANCEL, PAGE_ALL, 766, 539, 110);
        ui.anchor = FIXED;
        STATUS.store(
            ui.ctl[&ID_STATUS].0 as isize,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Options page
        let o = PAGE_OPT;
        ui.anchor = FIXED;
        let s = ui.shared.lock().unwrap().settings.clone();
        label(ui, "Whisper model", o, 28, 76, 180);
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "",
            BTN,
            B_MODEL,
            o,
            (220, 72, 420, 22),
        );
        let vals = [
            s.threads.to_string(),
            s.language,
            s.bind,
            s.port.to_string(),
            s.public_host,
            if s.bitrate_k == 0 {
                "Auto".to_string()
            } else {
                s.bitrate_k.to_string()
            },
            s.export_dir,
        ];
        for (i, v) in vals.iter().enumerate() {
            let y = 72 + (i as i32 + 1) * 32;
            label(ui, OPTION_LABELS[i], o, 28, y + 4, 180);
            let h = field(
                ui,
                instance,
                EDIT,
                OPT_BASE + i as i32,
                o,
                (220, y, 420, 22),
            );
            set_text(h, v);
        }
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "Require feed token",
            BTN,
            B_TOKEN,
            o,
            (220, 368, 200, 20),
        );
        label(ui, "Channels", o, 28, 332, 180);
        child(
            ui,
            instance,
            ui.hwnd,
            bt,
            "",
            BTN,
            B_CHANNELS,
            o,
            (220, 328, 420, 22),
        );
        ui.anchor = Anchor(1.0, 1.0, 0.0, 0.0);
        button(ui, instance, "Save", B_SAVE, o, 796, 72, 80);
        ui.anchor = FIXED;
    });
    set_page(false);
    update_buttons();
}

fn set_page(options: bool) {
    with(|ui| unsafe {
        ui.options = options;
        for (h, p) in &ui.pages {
            let show = *p == PAGE_ALL || (*p == PAGE_OPT) == options;
            let _ = ShowWindow(*h, if show { SW_SHOW } else { SW_HIDE });
        }
        let _ = InvalidateRect(ui.ctl[&B_TAB_MAIN], None, true);
        let _ = InvalidateRect(ui.ctl[&B_TAB_OPT], None, true);
        let _ = InvalidateRect(ui.hwnd, None, true);
    });
}

fn selected(ui: &Ui, id: i32) -> Option<usize> {
    let i = unsafe { SendMessageW(ui.ctl[&id], LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0).then_some(i as usize)
}

fn current(ui: &Ui) -> (Option<u32>, Option<String>) {
    let show = selected(ui, ID_SHOWS).and_then(|i| ui.show_ids.get(i).copied());
    let ep = selected(ui, ID_EPS).and_then(|i| ui.ep_guids.get(i).cloned());
    (show, ep)
}

fn state_tag(s: State) -> &'static str {
    match s {
        State::New => "NEW  ",
        State::Downloaded => "DL   ",
        State::Transcribed => "ASK AI",
        State::Ready => "READY",
        State::Delivered => "SENT ",
    }
}

/// Fill a list; an empty list is hidden so its clip parent shows
/// `empty` instead, like "No players are currently connected to the server."
fn fill_list(h: HWND, items: &[String], keep: Option<usize>, empty: &str) {
    unsafe {
        if let Ok(clip) = GetParent(h) {
            let w = wide(if items.is_empty() { empty } else { "" });
            let _ = SetWindowTextW(clip, PCWSTR(w.as_ptr()));
            let _ = ShowWindow(h, if items.is_empty() { SW_HIDE } else { SW_SHOWNA });
            let _ = InvalidateRect(clip, None, true);
        }
        SendMessageW(h, WM_SETREDRAW, WPARAM(0), LPARAM(0));
        SendMessageW(h, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for it in items {
            let w = wide(it);
            SendMessageW(h, LB_ADDSTRING, WPARAM(0), LPARAM(w.as_ptr() as isize));
        }
        if let Some(k) = keep.filter(|k| *k < items.len()) {
            SendMessageW(h, LB_SETCURSEL, WPARAM(k), LPARAM(0));
        }
        SendMessageW(h, WM_SETREDRAW, WPARAM(1), LPARAM(0));
        let _ = InvalidateRect(h, None, true);
    }
}

fn feed_link(ui: &Ui, sh: &Shared, id: u32) -> String {
    let host = if sh.settings.public_host.trim().is_empty() {
        let port = ui
            .server_addr
            .rsplit(':')
            .next()
            .unwrap_or("8737")
            .to_string();
        match ui.lan {
            Some(ip) => format!("{ip}:{port}"),
            None => {
                return "This PC's network address was not found. Enter it as \"PC address for phone\" in Options.".into()
            }
        }
    } else if sh.settings.public_host.contains(':') {
        sh.settings.public_host.trim().to_string()
    } else {
        format!("{}:{}", sh.settings.public_host.trim(), sh.settings.port)
    };
    let tok = match sh.lib.show(id) {
        Some(s) if sh.settings.require_token => format!("?token={}", s.token),
        _ => String::new(),
    };
    format!("http://{host}/feed/{id}.xml{tok}")
}

fn refresh_lists() {
    with(|ui| {
        let keep_show = selected(ui, ID_SHOWS);
        let keep_ep = selected(ui, ID_EPS);
        let sh = ui.shared.lock().unwrap();
        ui.show_ids = sh.lib.shows.iter().map(|s| s.id).collect();
        let names: Vec<String> = sh
            .lib
            .shows
            .iter()
            .map(|s| {
                let ready = s
                    .episodes
                    .iter()
                    .filter(|e| e.state == State::Ready)
                    .count();
                format!(
                    "{}  ({} ready)",
                    if s.title.is_empty() {
                        &s.feed_url
                    } else {
                        &s.title
                    },
                    ready
                )
            })
            .collect();
        let keep_show = keep_show.or(if names.is_empty() { None } else { Some(0) });
        fill_list(
            ui.ctl[&ID_SHOWS],
            &names,
            keep_show,
            "No shows yet. Paste a show's RSS link below and press Add show.",
        );
        let show = keep_show
            .and_then(|i| ui.show_ids.get(i))
            .and_then(|id| sh.lib.show(*id));
        let (eps, guids): (Vec<String>, Vec<String>) = show
            .map(|s| {
                s.episodes
                    .iter()
                    .map(|e| {
                        (
                            format!("[{}] {}", state_tag(e.state), e.title),
                            e.guid.clone(),
                        )
                    })
                    .unzip()
            })
            .unwrap_or_default();
        ui.ep_guids = guids;
        let none = if show.is_some() {
            "This show has no episodes yet."
        } else {
            "No show is selected."
        };
        fill_list(ui.ctl[&ID_EPS], &eps, keep_ep, none);
        let link = show.map(|s| feed_link(ui, &sh, s.id)).unwrap_or_default();
        set_text(ui.ctl[&ID_FEED], &link);
        if let Some(last) = sh.log.last() {
            set_text(ui.ctl[&ID_STATUS], last);
        }
    });
    show_details();
    update_buttons();
}

fn show_details() {
    with(|ui| {
        let (show, guid) = current(ui);
        let sh = ui.shared.lock().unwrap();
        let ep = show
            .and_then(|s| sh.lib.show(s))
            .and_then(|s| s.episodes.iter().find(|e| Some(&e.guid) == guid.as_ref()));
        let text = match ep {
            None if !ui.missing.is_empty() => ui.missing.clone(),
            None => {
                "Add a show with its RSS link, pick an episode, then press Download + transcribe."
                    .to_string()
            }
            Some(e) => {
                let mut t = format!("{}\n{}\n\n", e.title, e.pub_date);
                if !e.record.sha256.is_empty() {
                    t.push_str(&format!(
                        "Length {}\nSHA-256 {}\n",
                        format_time(e.record.duration),
                        e.record.sha256
                    ));
                }
                if !e.record.codec.is_empty() {
                    t.push_str(&format!("{}\n", source_line(e, &sh.settings)));
                }
                match e.state {
                    State::New => t.push_str("\nNot downloaded yet."),
                    State::Downloaded => t.push_str("\nDownloaded. Press Download + transcribe to transcribe."),
                    State::Transcribed => t.push_str("\nTranscribed. Copy the AI prompt, paste it into your AI, then paste its reply below and press Check cuts."),
                    State::Ready | State::Delivered => {
                        t.push_str(&format!(
                            "\nCuts ({} removed):\n",
                            format_time(removed_seconds(&e.record.cuts))
                        ));
                        for c in &e.record.cuts {
                            t.push_str(&format!("  {} - {}\n", format_time(c.start), format_time(c.end)));
                        }
                        t.push_str(if e.state == State::Ready {
                            "\nIn the feed. It is deleted after one full download."
                        } else {
                            "\nDownloaded once and deleted. Regenerate to put it back in the feed."
                        });
                    }
                }
                t
            }
        };
        drop(sh);
        if ui.pending.is_none() {
            set_text(ui.ctl[&ID_VIEW], &text);
        }
    });
}

/// e.g. "Source mp3 55 kbps stereo (identical channels) -> Opus 32 kbps mono (Auto)".
fn source_line(e: &store::Episode, s: &Settings) -> String {
    let r = &e.record;
    let mut src = r.codec.clone();
    if let Some(k) = r.source_kbps {
        src.push_str(&format!(" {k} kbps"));
    }
    src.push_str(&match (r.channels, r.identical) {
        (0, _) => String::new(),
        (1, _) => " mono".into(),
        (2, Some(true)) => " stereo (identical channels)".into(),
        (2, Some(false)) => " stereo".into(),
        (2, None) => " stereo (not checked)".into(),
        (n, _) => format!(" {n} channels"),
    });
    let done = r.opus_kbps > 0 && matches!(e.state, State::Ready | State::Delivered);
    let (kbps, mono, how) = if done {
        (r.opus_kbps, r.out_mono, String::new())
    } else {
        let t = pipeline::target(s, e);
        let how = match (t.auto_kbps, t.auto_channels) {
            (true, true) => " (Auto)",
            (true, false) => " (Auto bitrate)",
            (false, true) => " (Auto channels)",
            (false, false) => " (set in Options)",
        };
        (t.kbps, t.mono, how.to_string())
    };
    format!(
        "Source {src} \u{2192} Opus {kbps} kbps {}{how}",
        if mono { "mono" } else { "stereo" }
    )
}

fn update_buttons() {
    with(|ui| unsafe {
        let (show, guid) = current(ui);
        let sh = ui.shared.lock().unwrap();
        let st = show
            .and_then(|s| sh.lib.show(s))
            .and_then(|s| s.episodes.iter().find(|e| Some(&e.guid) == guid.as_ref()))
            .map(|e| e.state);
        drop(sh);
        let idle = !ui.busy;
        let en = |id: i32, on: bool| {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(ui.ctl[&id], on);
        };
        en(B_ADD, idle);
        en(B_REFRESH, idle && show.is_some());
        en(
            B_PROCESS,
            idle && matches!(st, Some(State::New | State::Downloaded)),
        );
        en(B_PROMPT, st == Some(State::Transcribed));
        en(B_CHECK, st == Some(State::Transcribed));
        en(
            B_APPROVE,
            idle && ui.pending.is_some() && st == Some(State::Transcribed),
        );
        en(B_REGEN, idle && st == Some(State::Delivered));
        en(B_EXPORT, idle && st == Some(State::Ready));
        en(B_COPYFEED, show.is_some());
        en(B_CANCEL, ui.busy);
        SetWindowLongPtrW(ui.ctl[&B_TOKEN], GWLP_USERDATA, ui.token_on as isize);
        let ch = wide(match ui.channels {
            store::Channels::Auto => {
                "Auto (mono when the source is mono or both channels are the same)"
            }
            c => c.label(),
        });
        let _ = SetWindowTextW(ui.ctl[&B_CHANNELS], PCWSTR(ch.as_ptr()));
        let _ = InvalidateRect(ui.ctl[&B_TOKEN], None, false);
        let _ = InvalidateRect(ui.ctl[&B_CHANNELS], None, false);
        let model = if ui.models.is_empty() {
            "No ggml-*.bin model next to Podcast Trim.exe".to_string()
        } else {
            let cur = ui
                .models
                .iter()
                .find(|m| m.eq_ignore_ascii_case(&ui.model))
                .unwrap_or(&ui.models[0]);
            cur.clone()
        };
        let mw = wide(&model);
        let _ = SetWindowTextW(ui.ctl[&B_MODEL], PCWSTR(mw.as_ptr()));
    });
}

/// Run `job` on a worker thread with the shared state, then refresh.
fn background(
    label: &str,
    job: impl FnOnce(&SharedRef) -> Result<String, String> + Send + 'static,
) {
    CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    PROGRESS.store(0, std::sync::atomic::Ordering::Relaxed);
    *STAGE.lock().unwrap() = label.to_string();
    let Some((shared, hraw)) = with(|ui| {
        ui.busy = true;
        status(ui, &format!("{label}..."));
        (ui.shared.clone(), ui.hwnd.0 as isize)
    }) else {
        return;
    };
    update_buttons();
    std::thread::spawn(move || {
        let msg = match job(&shared) {
            Ok(m) => m,
            Err(e) if e == CANCELLED => {
                "Cancelled. Steps that had already finished are kept.".to_string()
            }
            Err(e) => format!("Error: {e}"),
        };
        PROGRESS.store(0, std::sync::atomic::Ordering::Relaxed);
        STAGE.lock().unwrap().clear();
        {
            let mut sh = shared.lock().unwrap();
            sh.note(msg);
            sh.save();
        }
        unsafe {
            let _ = PostMessageW(HWND(hraw as _), WM_REFRESH, WPARAM(1), LPARAM(0));
        }
    });
}

/// Take a copy of an episode, work on it without the lock, write it back.
fn episode_job(
    show: u32,
    guid: String,
    work: impl FnOnce(&Settings, &Paths, &mut store::Episode) -> Result<String, String> + Send + 'static,
) -> impl FnOnce(&SharedRef) -> Result<String, String> + Send + 'static {
    move |shared| {
        let (settings, root, mut ep) = {
            let sh = shared.lock().unwrap();
            let ep = sh
                .lib
                .show(show)
                .and_then(|s| s.episodes.iter().find(|e| e.guid == guid))
                .cloned()
                .ok_or("episode is gone")?;
            (sh.settings.clone(), sh.paths.root.clone(), ep)
        };
        let settings = podcast_trim::tools::resolve(&settings, &podcast_trim::tools::exe_dir())?;
        let paths = Paths::new(root);
        let res = work(&settings, &paths, &mut ep);
        let mut sh = shared.lock().unwrap();
        if let Some(slot) = sh
            .lib
            .show_mut(show)
            .and_then(|s| s.episodes.iter_mut().find(|e| e.guid == guid))
        {
            *slot = ep;
        }
        res
    }
}

fn on_command(id: i32, code: u32) {
    if code == LBN_SELCHANGE {
        if id == ID_SHOWS {
            with(|ui| unsafe {
                SendMessageW(ui.ctl[&ID_EPS], LB_SETCURSEL, WPARAM(usize::MAX), LPARAM(0));
            });
            refresh_lists();
        }
        with(|ui| ui.pending = None);
        show_details();
        update_buttons();
        return;
    }
    let Some((show, guid, hwnd)) = with(|ui| {
        let (s, g) = current(ui);
        (s, g, ui.hwnd)
    }) else {
        return;
    };
    match id {
        B_ADD => {
            let url = with(|ui| get_text(ui.ctl[&ID_URL]))
                .unwrap_or_default()
                .trim()
                .to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                with(|ui| {
                    status(
                        ui,
                        "Paste the show's RSS link (starting with http:// or https://) first.",
                    )
                });
                return;
            }
            background("Reading feed", move |shared| {
                let xml = net::get(&url)?;
                let feed = rss::parse(&String::from_utf8_lossy(&xml))?;
                let mut sh = shared.lock().unwrap();
                if sh.lib.shows.iter().any(|s| s.feed_url == url) {
                    return Err("That show is already in the list.".into());
                }
                let id = sh.lib.add_show(feed.title.clone(), url);
                let show = sh.lib.show_mut(id).unwrap();
                let n = pipeline::merge_feed(show, &feed);
                Ok(format!(
                    "Added {} with {}.",
                    feed.title,
                    store::plural(n, "episode")
                ))
            });
            with(|ui| set_text(ui.ctl[&ID_URL], ""));
        }
        B_REFRESH => {
            let Some(show) = show else { return };
            background("Checking for new episodes", move |shared| {
                let url = shared
                    .lock()
                    .unwrap()
                    .lib
                    .show(show)
                    .map(|s| s.feed_url.clone())
                    .ok_or("show is gone")?;
                let feed = rss::parse(&String::from_utf8_lossy(&net::get(&url)?))?;
                let mut sh = shared.lock().unwrap();
                let s = sh.lib.show_mut(show).ok_or("show is gone")?;
                let n = pipeline::merge_feed(s, &feed);
                Ok(if n == 0 {
                    "No new episodes.".to_string()
                } else {
                    format!("{}.", store::plural(n, "new episode"))
                })
            });
        }
        B_PROCESS => {
            let (Some(show), Some(guid)) = (show, guid) else {
                return;
            };
            background(
                "Downloading and transcribing (this takes a while on the CPU)",
                episode_job(show, guid, move |s, p, ep| {
                    if ep.state == State::New {
                        pipeline::download(&SysEnv, s, p, show, ep)?;
                    }
                    pipeline::transcribe(&SysEnv, s, p, show, ep)?;
                    Ok(format!("Transcribed {}. Press Copy AI prompt.", ep.title))
                }),
            );
        }
        B_PROMPT => {
            let text = with(|ui| {
                let sh = ui.shared.lock().unwrap();
                let s = show.and_then(|s| sh.lib.show(s))?;
                let e = s.episodes.iter().find(|e| Some(&e.guid) == guid.as_ref())?;
                Some(prompt::build(
                    &s.title,
                    &e.title,
                    &format_time(e.record.duration),
                    &e.record.transcript,
                ))
            })
            .flatten();
            if let Some(t) = text {
                let ok = copy_to_clipboard(hwnd, &t);
                with(|ui| {
                    status(
                        ui,
                        if ok {
                            "Prompt and transcript copied. Paste them into your AI, then paste its reply in the box on the right."
                        } else {
                            "Could not open the clipboard."
                        },
                    )
                });
            }
        }
        B_CHECK => {
            with(|ui| {
                let reply = get_text(ui.ctl[&ID_REPLY]);
                let sh = ui.shared.lock().unwrap();
                let Some(ep) = show
                    .and_then(|s| sh.lib.show(s))
                    .and_then(|s| s.episodes.iter().find(|e| Some(&e.guid) == guid.as_ref()))
                    .cloned()
                else {
                    return;
                };
                let paths = Paths::new(sh.paths.root.clone());
                let settings = sh.settings.clone();
                drop(sh);
                match pipeline::check_reply(&ep, &reply) {
                    Ok(cuts) => {
                        let args = ffargs::cut_args(
                            &paths.original(show.unwrap(), &ep),
                            &paths.cleaned(show.unwrap(), &ep),
                            &cuts,
                            ep.record.duration,
                            pipeline::target(&settings, &ep).kbps,
                            pipeline::target(&settings, &ep).mono,
                        );
                        let mut t = format!(
                            "{} cut(s), {} removed:\n",
                            cuts.len(),
                            format_time(removed_seconds(&cuts))
                        );
                        for c in &cuts {
                            t.push_str(&format!(
                                "  {} - {}  ({})\n",
                                format_time(c.start),
                                format_time(c.end),
                                format_time(c.end - c.start)
                            ));
                        }
                        t.push_str("\nffmpeg command the app will run:\n");
                        t.push_str(&ffargs::display("ffmpeg.exe", &args));
                        t.push_str("\n\nPress Approve and cut to go ahead.");
                        set_text(ui.ctl[&ID_VIEW], &t);
                        ui.pending = Some((show.unwrap(), ep.guid.clone(), cuts));
                        status(ui, "Cut list checked. Review it in Details.");
                    }
                    Err(e) => {
                        ui.pending = None;
                        status(ui, &format!("The reply was not accepted: {e}"));
                    }
                }
            });
            update_buttons();
        }
        B_APPROVE => {
            let Some((s, g, cuts)) = with(|ui| ui.pending.take()).flatten() else {
                return;
            };
            if Some(s) != show || Some(&g) != guid.as_ref() {
                return;
            }
            with(|ui| set_text(ui.ctl[&ID_REPLY], ""));
            background(
                "Cutting",
                episode_job(s, g, move |st, p, ep| {
                    pipeline::apply_cuts(&SysEnv, st, p, s, ep, cuts)?;
                    Ok(format!("{} is ready in the feed.", ep.title))
                }),
            );
        }
        B_REGEN => {
            let (Some(show), Some(guid)) = (show, guid) else {
                return;
            };
            background(
                "Downloading the original again and checking its hash",
                episode_job(show, guid, move |s, p, ep| {
                    pipeline::regenerate(&SysEnv, s, p, show, ep)?;
                    Ok(format!(
                        "{} rebuilt from the saved cuts and back in the feed.",
                        ep.title
                    ))
                }),
            );
        }
        B_EXPORT => {
            let (Some(show), Some(guid)) = (show, guid) else {
                return;
            };
            // Folder picker first (modal, re-enters the window procedure),
            // starting in the last folder used.
            let last = with(|ui| ui.shared.lock().unwrap().settings.export_dir.clone())
                .unwrap_or_default();
            let Some(picked) = (unsafe { pick_folder(hwnd, &last) }) else {
                return;
            };
            with(|ui| {
                let mut sh = ui.shared.lock().unwrap();
                sh.settings.export_dir = picked.clone();
                let _ = save_json(&sh.paths.settings(), &sh.settings);
                if let Some(h) = ui.ctl.get(&(OPT_BASE + 6)) {
                    set_text(*h, &picked);
                }
                let dir = sh.settings.export_dir.clone();
                let ep = sh
                    .lib
                    .show(show)
                    .and_then(|s| s.episodes.iter().find(|e| e.guid == guid))
                    .cloned();
                let paths = Paths::new(sh.paths.root.clone());
                drop(sh);
                let msg = match (dir.trim().is_empty(), ep) {
                    (true, _) => "No folder was chosen.".to_string(),
                    (false, Some(ep)) => {
                        match pipeline::export(&paths, show, &ep, Path::new(dir.trim())) {
                            Ok(p) => format!("Saved {}", p.display()),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    _ => String::new(),
                };
                status(ui, &msg);
            });
        }
        B_COPYFEED => {
            with(|ui| {
                let link = get_text(ui.ctl[&ID_FEED]);
                copy_to_clipboard(hwnd, &link);
                status(ui, "Feed link copied. Add it to your podcast app.");
            });
        }
        B_CANCEL => {
            cancel_job();
            with(|ui| status(ui, "Cancelling..."));
        }
        B_TAB_OPT => set_page(true),
        B_TAB_MAIN => set_page(false),
        B_MIN => unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        },
        B_CLOSE => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        B_CHANNELS => {
            let info = with(|ui| (ui.channels, ui.ctl[&B_CHANNELS]));
            if let Some((cur, btn)) = info {
                let items = store::Channels::ALL
                    .iter()
                    .map(|c| c.label().to_string())
                    .collect();
                unsafe { open_dropdown(btn, B_CHANNELS, items, cur.label()) };
            }
        }
        B_MODEL => {
            let info = with(|ui| {
                ui.models = podcast_trim::tools::models(&podcast_trim::tools::exe_dir());
                (ui.models.clone(), ui.model.clone(), ui.ctl[&B_MODEL])
            });
            if let Some((models, cur, btn)) = info {
                if !models.is_empty() {
                    unsafe { open_dropdown(btn, B_MODEL, models, &cur) };
                }
            }
            update_buttons();
        }
        B_TOKEN => {
            with(|ui| ui.token_on = !ui.token_on);
            update_buttons();
        }
        B_SAVE => {
            with(|ui| {
                let v: Vec<String> = (0..OPTION_LABELS.len() as i32)
                    .map(|i| get_text(ui.ctl[&(OPT_BASE + i)]).trim().to_string())
                    .collect();
                let mut sh = ui.shared.lock().unwrap();
                let old_bind = (sh.settings.bind.clone(), sh.settings.port);
                let s = &mut sh.settings;
                s.whisper_model = ui.model.clone();
                s.channels = ui.channels;
                s.threads = v[0].parse().unwrap_or(s.threads);
                s.language = v[1].clone();
                s.bind = v[2].clone();
                s.port = v[3].parse().unwrap_or(s.port);
                s.public_host = v[4].clone();
                s.bitrate_k = if v[5].is_empty() || v[5].eq_ignore_ascii_case("auto") {
                    0
                } else {
                    v[5].trim_end_matches(['k', 'K'])
                        .parse::<u32>()
                        .ok()
                        .filter(|b| *b == 0 || (6..=256).contains(b))
                        .unwrap_or(s.bitrate_k)
                };
                s.export_dir = v[6].clone();
                s.require_token = ui.token_on;
                let restart = old_bind != (s.bind.clone(), s.port);
                let r = save_json(&sh.paths.settings(), &sh.settings);
                drop(sh);
                status(
                    ui,
                    &match (r, restart) {
                        (Err(e), _) => format!("Error: {e}"),
                        (Ok(()), true) => "Options saved. Restart Podcast Trim to use the new listen address or port.".into(),
                        (Ok(()), false) => "Options saved.".into(),
                    },
                );
            });
            set_page(false);
            refresh_lists();
        }
        _ => {}
    }
}

unsafe fn line(dc: HDC, x0: i32, y0: i32, x1: i32, y1: i32) {
    let _ = MoveToEx(dc, x0, y0, None);
    let _ = LineTo(dc, x1, y1);
}

/// 1px bevel: `raised` = light top/left, dark bottom/right.
unsafe fn bevel(dc: HDC, r: &RECT, raised: bool) {
    let (a, b) = if raised { (LIGHT, DARK) } else { (DARK, LIGHT) };
    let pa = CreatePen(PS_SOLID, 1, COLORREF(a));
    let pb = CreatePen(PS_SOLID, 1, COLORREF(b));
    let old = SelectObject(dc, pa);
    line(dc, r.left, r.bottom - 1, r.left, r.top);
    line(dc, r.left, r.top, r.right - 1, r.top);
    SelectObject(dc, pb);
    line(dc, r.right - 1, r.top, r.right - 1, r.bottom - 1);
    line(dc, r.right - 1, r.bottom - 1, r.left - 1, r.bottom - 1);
    SelectObject(dc, old);
    let _ = DeleteObject(pa);
    let _ = DeleteObject(pb);
}

unsafe fn text(
    dc: HDC,
    s: &str,
    r: &mut RECT,
    color: u32,
    flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    SetTextColor(dc, COLORREF(color));
    SetBkMode(dc, TRANSPARENT);
    DrawTextW(dc, &mut w, r, flags | DT_NOPREFIX);
}

unsafe fn fill(dc: HDC, r: &RECT, color: u32) {
    let b = CreateSolidBrush(COLORREF(color));
    FillRect(dc, r, b);
    let _ = DeleteObject(b);
}

/// Small solid triangle, pointing up or down, centred in `r`.
unsafe fn arrow(dc: HDC, r: &RECT, up: bool, color: u32) {
    let cx = (r.left + r.right) / 2;
    let cy = (r.top + r.bottom) / 2;
    let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
    let old = SelectObject(dc, pen);
    for i in 0..4 {
        let y = if up { cy - 2 + i } else { cy + 1 - i };
        line(dc, cx - i, y, cx + i + 1, y);
    }
    SelectObject(dc, old);
    let _ = DeleteObject(pen);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    with(|ui| {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right * 96 / ui.dpi, rc.bottom * 96 / ui.dpi);
        FillRect(dc, &rc, ui.brush_bg);
        // Window frame: light top/left, dark bottom/right.
        bevel(dc, &rc, true);
        let old = SelectObject(dc, ui.font);
        // Caption: icon and title on the same olive as the body.
        let d = ui.dpi;
        let _ = DrawIconEx(
            dc,
            scale(8, d),
            scale(5, d),
            ui.icon,
            scale(16, d),
            scale(16, d),
            0,
            None,
            DI_NORMAL,
        );
        let mut tr = sr(ui, 30, 2, 400, CAPTION_H - 2);
        text(
            dc,
            "Podcast Trim",
            &mut tr,
            TEXT,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        // Tab sheet.
        let sheet = sr(ui, 14, 52, cw - 28, ch - 52 - 10);
        bevel(dc, &sheet, true);
        let page = if ui.options { PAGE_OPT } else { PAGE_MAIN };
        for (i, (r, p, raised)) in ui.frames.iter().enumerate() {
            if *p == page {
                if !*raised {
                    let inner = RECT {
                        left: r.left + 1,
                        top: r.top + 1,
                        right: r.right - 1,
                        bottom: r.bottom - 1,
                    };
                    fill(dc, &inner, if i == ui.feed_frame { BG } else { FIELD });
                }
                bevel(dc, r, *raised);
            }
        }
        grip(dc, &rc);
        for (r, s, p, kind) in &ui.labels {
            if *p != page {
                continue;
            }
            let mut r = *r;
            if *kind == LabelKind::Header {
                fill(dc, &r, BG);
                bevel(dc, &r, true);
                r.left += scale(5, d);
            }
            text(dc, s, &mut r, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        }
        if ui.options {
            let mut r = sr(ui, 28, 404, cw - 60, 110);
            let note = format!(
                "Note: ffmpeg.exe, ffprobe.exe, whisper-cli.exe with its DLLs, and ggml-*.bin models must be in the same folder as Podcast Trim.exe.\nThe feed server is listening on {}. Use 0.0.0.0 to reach it from your phone on the same network, or 127.0.0.1 for this PC only. Address and port changes need a restart.\nPC address for phone: leave it blank to use this PC's detected address ({}). Opus bitrate: Auto picks it from each episode's source (as Opus Convert does), or enter a number from 6 to 256.",
                ui.server_addr,
                ui.lan.map(|i| i.to_string()).unwrap_or_else(|| "none found".into())
            );
            text(
                dc,
                &note,
                &mut r,
                DIM,
                DT_LEFT | windows::Win32::Graphics::Gdi::DT_WORDBREAK,
            );
        }
        SelectObject(dc, old);
    });
    let _ = EndPaint(hwnd, &ps);
}

thread_local! {
    /// Open model list: (items, hovered row, current row).
    static DROP: RefCell<(Vec<String>, i32, i32)> = const { RefCell::new((Vec::new(), -1, -1)) };
    /// Which field the open list belongs to.
    static DROP_OWNER: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Drop-down list under the model field, drawn like the VGUI combo boxes:
/// a dark-edged field-coloured list with a mustard highlight row.
unsafe fn open_dropdown(btn: HWND, owner: i32, items: Vec<String>, current: &str) {
    DROP_OWNER.with(|o| o.set(owner));
    let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
    let mut r = RECT::default();
    let _ = GetWindowRect(btn, &mut r);
    let cur = items
        .iter()
        .position(|m| m.eq_ignore_ascii_case(current))
        .map(|i| i as i32)
        .unwrap_or(-1);
    let h = scale(DROP_ROW, d) * items.len() as i32 + 2;
    DROP.with(|x| *x.borrow_mut() = (items, cur, cur));
    let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let main = HWND(MAIN.load(std::sync::atomic::Ordering::Relaxed) as _);
    let Ok(pop) = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        w!("PodcastTrimDrop"),
        None,
        WS_POPUP,
        r.left,
        r.bottom,
        r.right - r.left,
        h,
        main,
        None,
        instance,
        None,
    ) else {
        return;
    };
    let _ = ShowWindow(pop, SW_SHOWNOACTIVATE);
    windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(pop);
}

unsafe extern "system" fn drop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
    let row = scale(DROP_ROW, d);
    let x = (lp.0 & 0xffff) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let inside = x >= 0 && y >= 0 && x < rc.right && y < rc.bottom;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            fill(dc, &rc, FIELD);
            let old = SelectObject(dc, font());
            DROP.with(|x| {
                let (items, hover, cur) = &*x.borrow();
                for (i, it) in items.iter().enumerate() {
                    let i = i as i32;
                    let r = RECT {
                        left: 1,
                        top: 1 + i * row,
                        right: rc.right - 1,
                        bottom: 1 + (i + 1) * row,
                    };
                    let on = i == *hover;
                    if on {
                        fill(dc, &r, SEL);
                    }
                    let mut tr = RECT {
                        left: r.left + scale(5, d),
                        ..r
                    };
                    let color = if on {
                        rgb(255, 255, 255)
                    } else if i == *cur {
                        TAN
                    } else {
                        TEXT
                    };
                    text(dc, it, &mut tr, color, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
                }
            });
            SelectObject(dc, old);
            // Dark outline on all sides, as on the VGUI menus.
            let p = CreatePen(PS_SOLID, 1, COLORREF(DARK));
            let o = SelectObject(dc, p);
            line(dc, 0, 0, rc.right - 1, 0);
            line(dc, rc.right - 1, 0, rc.right - 1, rc.bottom - 1);
            line(dc, rc.right - 1, rc.bottom - 1, 0, rc.bottom - 1);
            line(dc, 0, rc.bottom - 1, 0, 0);
            SelectObject(dc, o);
            let _ = DeleteObject(p);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let h = if inside { (y - 1) / row } else { -1 };
            let changed = DROP.with(|x| {
                let mut g = x.borrow_mut();
                let c = g.1 != h;
                g.1 = h;
                c
            });
            if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let n = DROP.with(|x| x.borrow().0.len()) as i32;
            let pick = if inside { (y - 1) / row } else { -1 };
            if (0..n).contains(&pick) {
                let main = HWND(MAIN.load(std::sync::atomic::Ordering::Relaxed) as _);
                let owner = DROP_OWNER.with(|o| o.get()) as isize;
                let _ = PostMessageW(main, WM_MODEL_PICK, WPARAM(pick as usize), LPARAM(owner));
            }
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == 0x1B => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if HWND(lp.0 as _) != hwnd {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Standard Windows folder picker, starting in `start` if it exists.
unsafe fn pick_folder(owner: HWND, start: &str) -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let dlg: IFileOpenDialog =
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let opts = dlg.GetOptions().ok()?;
    dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
        .ok()?;
    let _ = dlg.SetTitle(w!("Export Opus to folder"));
    if !start.trim().is_empty() && Path::new(start.trim()).is_dir() {
        let ws = wide(start.trim());
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(ws.as_ptr()), None)
        {
            let _ = dlg.SetFolder(&item);
        }
    }
    dlg.Show(owner).ok()?;
    let item = dlg.GetResult().ok()?;
    let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let out = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    out
}

/// VGUI resize grip: three light diagonals with dark shadows in the corner.
unsafe fn grip(dc: HDC, rc: &RECT) {
    let (r, b) = (rc.right - 2, rc.bottom - 2);
    let pl = CreatePen(PS_SOLID, 1, COLORREF(LIGHT));
    let pd = CreatePen(PS_SOLID, 1, COLORREF(DARK));
    let old = SelectObject(dc, pl);
    for k in 0..3 {
        let o = 3 + k * 4;
        SelectObject(dc, pl);
        line(dc, r - o, b, r + 1, b - o - 1);
        SelectObject(dc, pd);
        line(dc, r - o + 1, b, r + 1, b - o);
    }
    SelectObject(dc, old);
    let _ = DeleteObject(pl);
    let _ = DeleteObject(pd);
}

/// GoldSource "Loading..." bar: sunken olive track, discrete khaki blocks.
unsafe extern "system" fn progress_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            fill(dc, &rc, FIELD);
            bevel(dc, &rc, false);
            let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
            let (inset, bw, gap) = (scale(4, d), scale(8, d), scale(4, d));
            let frac = PROGRESS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
            let (_, on) = podcast_trim::progress::blocks(rc.right - 2 * inset, bw, gap, frac);
            for i in 0..on {
                let x = inset + i * (bw + gap);
                let blk = RECT {
                    left: x,
                    top: inset,
                    right: x + bw,
                    bottom: rc.bottom - inset,
                };
                fill(dc, &blk, TAN);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn draw_list_item(dis: &DRAWITEMSTRUCT) {
    if dis.itemID == u32::MAX {
        return;
    }
    let dc = dis.hDC;
    let r = dis.rcItem;
    let sel = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let n = SendMessageW(
        dis.hwndItem,
        LB_GETTEXTLEN,
        WPARAM(dis.itemID as usize),
        LPARAM(0),
    )
    .0
    .max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    SendMessageW(
        dis.hwndItem,
        LB_GETTEXT,
        WPARAM(dis.itemID as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    );
    let label = String::from_utf16_lossy(&buf[..n]);
    let brush = CreateSolidBrush(COLORREF(if sel { SEL } else { FIELD }));
    FillRect(dc, &r, brush);
    let _ = DeleteObject(brush);
    {
        let old = SelectObject(dc, font());
        let mut tr = r;
        tr.left += 4;
        text(
            dc,
            &label,
            &mut tr,
            if sel { rgb(255, 255, 255) } else { TEXT },
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS,
        );
        SelectObject(dc, old);
    }
}

unsafe fn draw_button(dis: &DRAWITEMSTRUCT) {
    let dc = dis.hDC;
    let r = dis.rcItem;
    let pressed = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let disabled = (dis.itemState.0 & ODS_DISABLED.0) != 0;
    let id = dis.CtlID as i32;
    let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
    fill(dc, &r, BG);
    let old = SelectObject(dc, font());
    let n = GetWindowTextLengthW(dis.hwndItem);
    let mut buf = vec![0u16; n as usize + 1];
    let got = GetWindowTextW(dis.hwndItem, &mut buf);
    let label = String::from_utf16_lossy(&buf[..got as usize]);
    let flags = DT_LEFT | DT_SINGLELINE | DT_VCENTER;
    match id {
        B_MIN | B_CLOSE => {
            bevel(dc, &r, !pressed);
            let pen = CreatePen(PS_SOLID, 1, COLORREF(LIGHT));
            let o = SelectObject(dc, pen);
            let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
            if id == B_MIN {
                for k in 0..2 {
                    line(dc, cx - 4, cy + 3 + k, cx + 3, cy + 3 + k);
                }
            } else {
                for k in 0..2 {
                    line(dc, cx - 4 + k, cy - 4, cx + 4 + k, cy + 4);
                    line(dc, cx + 3 + k, cy - 4, cx - 5 + k, cy + 4);
                }
            }
            SelectObject(dc, o);
            let _ = DeleteObject(pen);
        }
        B_TAB_MAIN | B_TAB_OPT => {
            let active = with(|ui| ui.options)
                .map(|o| o == (id == B_TAB_OPT))
                .unwrap_or(id == B_TAB_MAIN);
            // Raised on three sides; the active tab joins the sheet below it.
            let mut t = r;
            if !active {
                t.top += scale(2, d);
            }
            let tab = RECT {
                bottom: r.bottom + 2,
                ..t
            };
            bevel(dc, &tab, true);
            if !active {
                let p = CreatePen(PS_SOLID, 1, COLORREF(LIGHT));
                let o = SelectObject(dc, p);
                line(dc, r.left, r.bottom - 1, r.right, r.bottom - 1);
                SelectObject(dc, o);
                let _ = DeleteObject(p);
            }
            let mut tr = t;
            tr.left += scale(6, d);
            text(dc, &label, &mut tr, if active { TAN } else { TEXT }, flags);
        }
        B_TOKEN => {
            let on = GetWindowLongPtrW(dis.hwndItem, GWLP_USERDATA) != 0;
            let s = scale(13, d);
            let bx = RECT {
                left: r.left,
                top: (r.top + r.bottom - s) / 2,
                right: r.left + s,
                bottom: (r.top + r.bottom + s) / 2,
            };
            fill(dc, &bx, FIELD);
            bevel(dc, &bx, false);
            if on {
                let p = CreatePen(PS_SOLID, 1, COLORREF(TEXT));
                let o = SelectObject(dc, p);
                for k in 0..2 {
                    line(dc, bx.left + 3, bx.top + 5 + k, bx.left + 5, bx.top + 7 + k);
                    line(
                        dc,
                        bx.left + 5,
                        bx.top + 7 + k,
                        bx.left + 10,
                        bx.top + 2 + k,
                    );
                }
                SelectObject(dc, o);
                let _ = DeleteObject(p);
            }
            let mut tr = r;
            tr.left += s + scale(7, d);
            text(
                dc,
                &label,
                &mut tr,
                if disabled {
                    DIM
                } else if on {
                    TAN
                } else {
                    TEXT
                },
                flags,
            );
        }
        B_MODEL | B_CHANNELS => {
            // Looks like a VGUI drop-down: sunken field with a small arrow.
            fill(dc, &r, FIELD);
            bevel(dc, &r, false);
            let mut tr = r;
            tr.left += scale(5, d);
            tr.right -= scale(20, d);
            text(
                dc,
                &label,
                &mut tr,
                TEXT,
                flags | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS,
            );
            let a = RECT {
                left: r.right - scale(18, d),
                ..r
            };
            // Small 5x3 arrow straight on the field, as in the HLDS combo boxes.
            let (cx, cy) = ((a.left + a.right) / 2, (a.top + a.bottom) / 2);
            let p = CreatePen(PS_SOLID, 1, COLORREF(DISABLED_TEXT));
            let o = SelectObject(dc, p);
            for i in 0..3 {
                line(dc, cx - 2 + i, cy - 1 + i, cx + 3 - i, cy - 1 + i);
            }
            SelectObject(dc, o);
            let _ = DeleteObject(p);
        }
        _ => {
            // Focused (default) button: black outline around the bevel.
            let focus = (dis.itemState.0 & ODS_FOCUS.0) != 0 && !disabled;
            let mut br = r;
            if focus {
                let p = CreatePen(PS_SOLID, 1, COLORREF(BLACK));
                let o = SelectObject(dc, p);
                line(dc, r.left, r.top, r.right - 1, r.top);
                line(dc, r.right - 1, r.top, r.right - 1, r.bottom - 1);
                line(dc, r.right - 1, r.bottom - 1, r.left, r.bottom - 1);
                line(dc, r.left, r.bottom - 1, r.left, r.top);
                SelectObject(dc, o);
                let _ = DeleteObject(p);
                br = RECT {
                    left: r.left + 1,
                    top: r.top + 1,
                    right: r.right - 1,
                    bottom: r.bottom - 1,
                };
            }
            bevel(dc, &br, !pressed);
            let mut tr = r;
            tr.left += scale(6, d);
            if pressed {
                tr.left += 1;
                tr.top += 2;
            }
            text(
                dc,
                &label,
                &mut tr,
                if disabled { DIM } else { TEXT },
                flags,
            );
        }
    }
    SelectObject(dc, old);
}

/// Parent that hides a list's or edit's own scroll bar; forwards everything
/// the main window needs (commands, owner-draw, colours).
unsafe extern "system" fn clip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND | WM_DRAWITEM | WM_MEASUREITEM | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX
        | WM_CTLCOLORSTATIC => {
            let main = GetAncestor(hwnd, GA_ROOT);
            wndproc(main, msg, wp, lp)
        }
        WM_ERASEBKGND => {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            fill(HDC(wp.0 as _), &rc, FIELD);
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let msg = get_text(hwnd);
            if !msg.is_empty() {
                let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                fill(dc, &rc, FIELD);
                rc.left += scale(5, d);
                rc.top += scale(4, d);
                rc.right -= scale(5, d);
                let old = SelectObject(dc, font());
                text(
                    dc,
                    &msg,
                    &mut rc,
                    TEXT,
                    DT_LEFT | windows::Win32::Graphics::Gdi::DT_WORDBREAK,
                );
                SelectObject(dc, old);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

thread_local! {
    /// Scroll bar being dragged: (bar, pixel offset of the grab inside the thumb).
    static DRAG: std::cell::Cell<Option<(isize, i32)>> = const { std::cell::Cell::new(None) };
    static LAST: RefCell<HashMap<isize, (i32, i32, u32, i32)>> = RefCell::new(HashMap::new());
}

fn scroll_target(bar: HWND) -> HWND {
    HWND(unsafe { GetWindowLongPtrW(bar, GWLP_USERDATA) } as _)
}

unsafe fn scroll_info(target: HWND) -> SCROLLINFO {
    let mut si = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..Default::default()
    };
    let _ = GetScrollInfo(target, SB_VERT, &mut si);
    si
}

/// (track top, track bottom, thumb top, thumb bottom) in bar pixels.
fn thumb_geometry(si: &SCROLLINFO, h: i32, btn: i32) -> (i32, i32, i32, i32) {
    let (t0, t1) = (btn, (h - btn).max(btn));
    let track = t1 - t0;
    let range = (si.nMax - si.nMin + 1).max(1);
    let page = (si.nPage as i32).clamp(0, range);
    if page == 0 || page >= range {
        return (t0, t1, t0, t1);
    }
    let size = (track * page / range).max(btn.min(track));
    let span = (range - page).max(1);
    let top = t0 + (track - size) * (si.nPos - si.nMin).clamp(0, span) / span;
    (t0, t1, top, top + size)
}

unsafe extern "system" fn scroll_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let btn = rc.right;
    let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let si = scroll_info(scroll_target(hwnd));
            // Lighter track with a dark left edge and a light right edge.
            fill(dc, &rc, TRACK);
            let pd = CreatePen(PS_SOLID, 1, COLORREF(DARK));
            let pl = CreatePen(PS_SOLID, 1, COLORREF(LIGHT));
            let o = SelectObject(dc, pd);
            line(dc, 0, 0, 0, rc.bottom);
            SelectObject(dc, pl);
            line(dc, rc.right - 1, 0, rc.right - 1, rc.bottom);
            SelectObject(dc, o);
            let _ = DeleteObject(pd);
            let _ = DeleteObject(pl);
            let up = RECT {
                left: 0,
                top: 0,
                right: btn,
                bottom: btn,
            };
            let down = RECT {
                left: 0,
                top: rc.bottom - btn,
                right: btn,
                bottom: rc.bottom,
            };
            for (b, u) in [(up, true), (down, false)] {
                fill(dc, &b, BG);
                bevel(dc, &b, true);
                arrow(dc, &b, u, TEXT);
            }
            let (_, _, a, z) = thumb_geometry(&si, rc.bottom, btn);
            if z - a < rc.bottom - 2 * btn {
                let th = RECT {
                    left: 0,
                    top: a,
                    right: btn,
                    bottom: z,
                };
                fill(dc, &th, BG);
                bevel(dc, &th, true);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_TIMER => {
            let si = scroll_info(scroll_target(hwnd));
            let key = hwnd.0 as isize;
            let now = (si.nMin, si.nMax, si.nPage, si.nPos);
            let changed = LAST.with(|l| l.borrow_mut().insert(key, now) != Some(now));
            if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let target = scroll_target(hwnd);
            let si = scroll_info(target);
            let (_, _, a, z) = thumb_geometry(&si, rc.bottom, btn);
            let code = if y < btn {
                SB_LINEUP
            } else if y >= rc.bottom - btn {
                SB_LINEDOWN
            } else if y < a {
                SB_PAGEUP
            } else if y >= z {
                SB_PAGEDOWN
            } else {
                DRAG.with(|d| d.set(Some((hwnd.0 as isize, y - a))));
                windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
                return LRESULT(0);
            };
            SendMessageW(target, WM_VSCROLL, WPARAM(code.0 as usize), LPARAM(0));
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some((h, off)) = DRAG.with(|d| d.get()) {
                if h == hwnd.0 as isize {
                    let target = scroll_target(hwnd);
                    let si = scroll_info(target);
                    let (t0, t1, a, z) = thumb_geometry(&si, rc.bottom, btn);
                    let free = (t1 - t0 - (z - a)).max(1);
                    let span = (si.nMax - si.nMin + 1 - si.nPage as i32).max(0);
                    let pos = si.nMin + ((y - off - t0).clamp(0, free) * span + free / 2) / free;
                    let w = (SB_THUMBTRACK.0 as usize) | ((pos as usize & 0xffff) << 16);
                    SendMessageW(target, WM_VSCROLL, WPARAM(w), LPARAM(0));
                    let w = (SB_THUMBPOSITION.0 as usize) | ((pos as usize & 0xffff) << 16);
                    SendMessageW(target, WM_VSCROLL, WPARAM(w), LPARAM(0));
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if DRAG.with(|d| d.take()).is_some() {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                SendMessageW(
                    scroll_target(hwnd),
                    WM_VSCROLL,
                    WPARAM(SB_ENDSCROLL.0 as usize),
                    LPARAM(0),
                );
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => {
            let mut pt = windows::Win32::Foundation::POINT {
                x: (lp.0 & 0xffff) as i16 as i32,
                y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
            };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            let cap = scale(CAPTION_H, DPI.load(std::sync::atomic::Ordering::Relaxed));
            if pt.y >= 0 && pt.y < cap {
                return LRESULT(HTCAPTION as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => {
            let mut pt = windows::Win32::Foundation::POINT {
                x: (lp.0 & 0xffff) as i16 as i32,
                y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
            };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let g = scale(16, d);
            if !IsZoomed(hwnd).as_bool() && pt.x >= rc.right - g && pt.y >= rc.bottom - g {
                return LRESULT(HTBOTTOMRIGHT as isize);
            }
            if pt.y >= 0 && pt.y < scale(CAPTION_H, d) {
                return LRESULT(HTCAPTION as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_GETMINMAXINFO => {
            let d = DPI.load(std::sync::atomic::Ordering::Relaxed);
            let mm = &mut *(lp.0 as *mut MINMAXINFO);
            mm.ptMinTrackSize.x = scale(WIN_W, d);
            mm.ptMinTrackSize.y = scale(WIN_H, d);
            LRESULT(0)
        }
        WM_SIZE => {
            layout();
            LRESULT(0)
        }
        WM_MODEL_PICK => {
            with(|ui| {
                if lp.0 as i32 == B_CHANNELS {
                    if let Some(c) = store::Channels::ALL.get(wp.0) {
                        ui.channels = *c;
                    }
                } else if let Some(m) = ui.models.get(wp.0) {
                    ui.model = m.clone();
                }
            });
            update_buttons();
            LRESULT(0)
        }
        WM_PROGRESS => {
            let pct = PROGRESS.load(std::sync::atomic::Ordering::Relaxed);
            let stage = STAGE.lock().unwrap().clone();
            with(|ui| unsafe {
                if ui.busy && !cancelled() && !stage.is_empty() {
                    status(ui, &format!("{stage}... {}%", pct / 10));
                }
                let _ = InvalidateRect(ui.ctl[&ID_PROGRESS], None, false);
            });
            LRESULT(0)
        }
        WM_MEASUREITEM => {
            let mis = &mut *(lp.0 as *mut windows::Win32::UI::Controls::MEASUREITEMSTRUCT);
            mis.itemHeight = scale(18, DPI.load(std::sync::atomic::Ordering::Relaxed)) as u32;
            LRESULT(1)
        }
        WM_DRAWITEM => {
            let dis = &*(lp.0 as *const DRAWITEMSTRUCT);
            if dis.CtlType == windows::Win32::UI::Controls::ODT_LISTBOX {
                draw_list_item(dis);
            } else {
                draw_button(dis);
            }
            LRESULT(1)
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            let dc = HDC(wp.0 as _);
            let is_status = lp.0 == STATUS.load(std::sync::atomic::Ordering::Relaxed);
            if lp.0 == FEED.load(std::sync::atomic::Ordering::Relaxed) {
                SetTextColor(dc, COLORREF(DISABLED_TEXT));
                SetBkColor(dc, COLORREF(BG));
                return LRESULT(BRUSH_BG.load(std::sync::atomic::Ordering::Relaxed));
            }
            SetTextColor(dc, COLORREF(TEXT));
            if is_status {
                SetBkColor(dc, COLORREF(BG));
                return LRESULT(BRUSH_BG.load(std::sync::atomic::Ordering::Relaxed));
            }
            SetBkColor(dc, COLORREF(FIELD));
            LRESULT(BRUSH_FIELD.load(std::sync::atomic::Ordering::Relaxed))
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let code = ((wp.0 >> 16) & 0xffff) as u32;
            if code == BN_CLICKED && id >= 200 || code == LBN_SELCHANGE {
                on_command(id, code);
            }
            LRESULT(0)
        }
        WM_REFRESH => {
            if wp.0 == 1 {
                with(|ui| unsafe {
                    ui.busy = false;
                    let _ = InvalidateRect(ui.ctl[&ID_PROGRESS], None, false);
                });
            }
            refresh_lists();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
