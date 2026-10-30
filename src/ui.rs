//! Win32 window drawn in the style of GoldSource-era VGUI menus:
//! olive panels, khaki text, 1px light/dark bevels, flat beveled buttons.
//! Every button is owner-drawn; edits and lists are recoloured.

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
    FillRect, GetDC, GetDeviceCaps, InvalidateRect, LineTo, MoveToEx, ReleaseDC, SelectObject,
    SetBkColor, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HFONT, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_SELECTED};
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

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const WM_REFRESH: u32 = WM_APP + 1;

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
const B_OPTIONS: i32 = 209;
const B_SAVE: i32 = 210;
const B_BACK: i32 = 211;
const B_TOKEN: i32 = 212;
const OPT_BASE: i32 = 300;

const OPTION_LABELS: [&str; 11] = [
    "FFMPEG.EXE",
    "FFPROBE.EXE",
    "WHISPER-CLI.EXE",
    "WHISPER MODEL",
    "CPU THREADS",
    "LANGUAGE",
    "LISTEN ADDRESS",
    "PORT",
    "PC ADDRESS FOR PHONE",
    "MP3 BITRATE (KBPS)",
    "EXPORT FOLDER",
];

struct Ui {
    hwnd: HWND,
    shared: SharedRef,
    font: HFONT,
    title_font: HFONT,
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
    server_addr: String,
    frames: Vec<RECT>,
    labels: Vec<(RECT, String, bool)>,
}

static BRUSH_BG: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static BRUSH_FIELD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static STATUS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static FONT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn font() -> HFONT {
    HFONT(FONT.load(std::sync::atomic::Ordering::Relaxed) as _)
}

static DPI: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(96);

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
        net::download(url, dest)
    }
    fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        let out = std::process::Command::new(program)
            .args(args)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("Could not start {program}: {e}. Check the path in Options."))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(4).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(format!("{program} failed: {}", tail.join(" | ")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn scale(v: i32, dpi: i32) -> i32 {
    v * dpi / 96
}

fn make_font(px: i32, bold: bool) -> HFONT {
    // Tahoma without anti-aliasing reads like the old VGUI bitmap text.
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
        load_json(&paths.settings())
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
        let dc = GetDC(None);
        let dpi = GetDeviceCaps(dc, LOGPIXELSX).max(96);
        ReleaseDC(None, dc);
        DPI.store(dpi, std::sync::atomic::Ordering::Relaxed);
        let (w, h) = (scale(980, dpi), scale(660, dpi));
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let _ = AdjustWindowRect(&mut r, style, false);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PodcastTrimWindow"),
            w!("Podcast Trim"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
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
        let ui = Ui {
            hwnd,
            shared: shared.clone(),
            font: make_font(scale(11, dpi), false),
            title_font: make_font(scale(18, dpi), true),
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
            server_addr: String::new(),
            frames: vec![],
            labels: vec![],
        };
        BRUSH_BG.store(ui.brush_bg.0 as isize, std::sync::atomic::Ordering::Relaxed);
        BRUSH_FIELD.store(
            ui.brush_field.0 as isize,
            std::sync::atomic::Ordering::Relaxed,
        );
        FONT.store(ui.font.0 as isize, std::sync::atomic::Ordering::Relaxed);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        build_controls(instance);
        let hraw = hwnd.0 as isize;
        let notify: server::Notify = Arc::new(move || {
            let _ = PostMessageW(HWND(hraw as _), WM_REFRESH, WPARAM(0), LPARAM(0));
        });
        let started = server::start(shared.clone(), notify);
        with(|ui| match started {
            Ok(addr) => {
                ui.server_addr = addr.to_string();
                status(ui, &format!("Feed server listening on {addr}."));
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

unsafe fn child(
    ui: &mut Ui,
    instance: HINSTANCE,
    class: PCWSTR,
    text: &str,
    style: u32,
    id: i32,
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
        ui.hwnd,
        HMENU(id as _),
        instance,
        None,
    )
    .unwrap_or_default();
    SendMessageW(h, WM_SETFONT, WPARAM(ui.font.0 as usize), LPARAM(1));
    ui.ctl.insert(id, h);
    h
}

fn frame(ui: &mut Ui, r: (i32, i32, i32, i32)) {
    let d = ui.dpi;
    ui.frames.push(RECT {
        left: scale(r.0, d) - 1,
        top: scale(r.1, d) - 1,
        right: scale(r.0 + r.2, d) + 1,
        bottom: scale(r.1 + r.3, d) + 1,
    });
}

fn label(ui: &mut Ui, text: &str, x: i32, y: i32, w: i32, tan: bool) {
    let d = ui.dpi;
    ui.labels.push((
        RECT {
            left: scale(x, d),
            top: scale(y, d),
            right: scale(x + w, d),
            bottom: scale(y + 16, d),
        },
        text.into(),
        tan,
    ));
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
    | WS_VSCROLL.0
    | WS_TABSTOP.0;

fn build_controls(instance: HINSTANCE) {
    with(|ui| unsafe {
        let (l, bt) = (w!("LISTBOX"), w!("BUTTON"));
        let e = w!("EDIT");
        // Main page
        label(ui, "SHOWS", 16, 52, 280, true);
        child(ui, instance, l, "", LIST, ID_SHOWS, (16, 72, 280, 330));
        frame(ui, (16, 72, 280, 330));
        label(ui, "SHOW RSS LINK", 16, 412, 280, false);
        child(ui, instance, e, "", EDIT, ID_URL, (16, 432, 280, 20));
        frame(ui, (16, 432, 280, 20));
        child(ui, instance, bt, "Add show", BTN, B_ADD, (16, 462, 136, 24));
        child(
            ui,
            instance,
            bt,
            "Check for new",
            BTN,
            B_REFRESH,
            (160, 462, 136, 24),
        );

        label(ui, "EPISODES", 312, 52, 300, true);
        child(ui, instance, l, "", LIST, ID_EPS, (312, 72, 320, 330));
        frame(ui, (312, 72, 320, 330));
        child(
            ui,
            instance,
            bt,
            "Download + transcribe",
            BTN,
            B_PROCESS,
            (312, 412, 158, 24),
        );
        child(
            ui,
            instance,
            bt,
            "Copy AI prompt",
            BTN,
            B_PROMPT,
            (474, 412, 158, 24),
        );
        child(
            ui,
            instance,
            bt,
            "Regenerate",
            BTN,
            B_REGEN,
            (312, 442, 158, 24),
        );
        child(
            ui,
            instance,
            bt,
            "Export MP3...",
            BTN,
            B_EXPORT,
            (474, 442, 158, 24),
        );

        label(ui, "DETAILS", 648, 52, 316, true);
        child(
            ui,
            instance,
            e,
            "",
            MEMO | ES_READONLY as u32,
            ID_VIEW,
            (648, 72, 316, 210),
        );
        frame(ui, (648, 72, 316, 210));
        label(ui, "PASTE THE AI'S REPLY", 648, 292, 316, false);
        child(ui, instance, e, "", MEMO, ID_REPLY, (648, 312, 316, 90));
        frame(ui, (648, 312, 316, 90));
        child(
            ui,
            instance,
            bt,
            "Check cuts",
            BTN,
            B_CHECK,
            (648, 412, 156, 24),
        );
        child(
            ui,
            instance,
            bt,
            "Approve and cut",
            BTN,
            B_APPROVE,
            (808, 412, 156, 24),
        );

        label(ui, "FEED LINK FOR YOUR PODCAST APP", 16, 504, 400, true);
        child(
            ui,
            instance,
            e,
            "",
            EDIT | ES_READONLY as u32,
            ID_FEED,
            (16, 524, 780, 20),
        );
        frame(ui, (16, 524, 780, 20));
        child(
            ui,
            instance,
            bt,
            "Copy feed link",
            BTN,
            B_COPYFEED,
            (808, 522, 156, 24),
        );
        child(
            ui,
            instance,
            bt,
            "Options",
            BTN,
            B_OPTIONS,
            (808, 616, 156, 28),
        );
        child(
            ui,
            instance,
            w!("STATIC"),
            "",
            0x0C | 0x80,
            ID_STATUS,
            (16, 566, 948, 40),
        );
        STATUS.store(
            ui.ctl[&ID_STATUS].0 as isize,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Options page (hidden until opened)
        let s = ui.shared.lock().unwrap().settings.clone();
        let vals = [
            s.ffmpeg,
            s.ffprobe,
            s.whisper_cli,
            s.whisper_model,
            s.threads.to_string(),
            s.language,
            s.bind,
            s.port.to_string(),
            s.public_host,
            s.bitrate_k.to_string(),
            s.export_dir,
        ];
        for (i, v) in vals.iter().enumerate() {
            let y = 80 + i as i32 * 34;
            let h = child(
                ui,
                instance,
                e,
                v,
                EDIT,
                OPT_BASE + i as i32,
                (220, y, 600, 20),
            );
            let _ = ShowWindow(h, SW_HIDE);
        }
        let h = child(
            ui,
            instance,
            bt,
            "",
            BTN,
            B_TOKEN,
            (220, 80 + 11 * 34, 260, 24),
        );
        let _ = ShowWindow(h, SW_HIDE);
        for id in [B_SAVE, B_BACK] {
            let (t, x) = if id == B_SAVE {
                ("Save", 648)
            } else {
                ("Back", 808)
            };
            let h = child(ui, instance, bt, t, BTN, id, (x, 616, 156, 28));
            let _ = ShowWindow(h, SW_HIDE);
        }
    });
    update_buttons();
}

const MAIN_IDS: [i32; 20] = [
    ID_SHOWS, ID_EPS, ID_URL, ID_VIEW, ID_REPLY, ID_FEED, B_ADD, B_REFRESH, B_PROCESS, B_PROMPT,
    B_CHECK, B_APPROVE, B_REGEN, B_EXPORT, B_COPYFEED, B_OPTIONS, 0, 0, 0, 0,
];

fn set_page(options: bool) {
    with(|ui| unsafe {
        ui.options = options;
        for id in MAIN_IDS.iter().filter(|i| **i != 0) {
            let _ = ShowWindow(ui.ctl[id], if options { SW_HIDE } else { SW_SHOW });
        }
        for i in 0..OPTION_LABELS.len() as i32 {
            let _ = ShowWindow(
                ui.ctl[&(OPT_BASE + i)],
                if options { SW_SHOW } else { SW_HIDE },
            );
        }
        for id in [B_TOKEN, B_SAVE, B_BACK] {
            let _ = ShowWindow(ui.ctl[&id], if options { SW_SHOW } else { SW_HIDE });
        }
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

fn fill_list(h: HWND, items: &[String], keep: Option<usize>) {
    unsafe {
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
        format!("<this-pc-address>:{port}")
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
        fill_list(ui.ctl[&ID_SHOWS], &names, keep_show);
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
        fill_list(ui.ctl[&ID_EPS], &eps, keep_ep);
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
        let _ = SetWindowTextW(
            ui.ctl[&B_TOKEN],
            if ui.token_on {
                w!("[X] Require feed token")
            } else {
                w!("[  ] Require feed token")
            },
        );
    });
}

/// Run `job` on a worker thread with the shared state, then refresh.
fn background(
    label: &str,
    job: impl FnOnce(&SharedRef) -> Result<String, String> + Send + 'static,
) {
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
            Err(e) => format!("Error: {e}"),
        };
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
                Ok(format!("Added {} with {n} episodes.", feed.title))
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
                Ok(format!("{n} new episode(s)."))
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
                            settings.bitrate_k,
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
                        t.push_str(&ffargs::display(&settings.ffmpeg, &args));
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
            with(|ui| {
                let sh = ui.shared.lock().unwrap();
                let dir = sh.settings.export_dir.clone();
                let ep = sh
                    .lib
                    .show(show)
                    .and_then(|s| s.episodes.iter().find(|e| e.guid == guid))
                    .cloned();
                let paths = Paths::new(sh.paths.root.clone());
                drop(sh);
                let msg = match (dir.trim().is_empty(), ep) {
                    (true, _) => "Set an export folder in Options first.".to_string(),
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
                status(ui, "Feed link copied. Replace <this-pc-address> with this PC's address if shown, or set it in Options.");
            });
        }
        B_OPTIONS => set_page(true),
        B_BACK => set_page(false),
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
                s.ffmpeg = v[0].clone();
                s.ffprobe = v[1].clone();
                s.whisper_cli = v[2].clone();
                s.whisper_model = v[3].clone();
                s.threads = v[4].parse().unwrap_or(s.threads);
                s.language = v[5].clone();
                s.bind = v[6].clone();
                s.port = v[7].parse().unwrap_or(s.port);
                s.public_host = v[8].clone();
                s.bitrate_k = v[9]
                    .parse::<u32>()
                    .ok()
                    .filter(|b| (32..=320).contains(b))
                    .unwrap_or(s.bitrate_k);
                s.export_dir = v[10].clone();
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

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    with(|ui| {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        FillRect(dc, &rc, ui.brush_bg);
        // Outer window bevel and title strip, like a VGUI frame.
        let inner = RECT {
            left: rc.left + 4,
            top: rc.top + 4,
            right: rc.right - 4,
            bottom: rc.bottom - 4,
        };
        bevel(dc, &inner, true);
        let old = SelectObject(dc, ui.title_font);
        let mut tr = RECT {
            left: scale(16, ui.dpi),
            top: scale(14, ui.dpi),
            right: rc.right - 16,
            bottom: scale(42, ui.dpi),
        };
        text(
            dc,
            if ui.options {
                "PODCAST TRIM  -  OPTIONS"
            } else {
                "PODCAST TRIM"
            },
            &mut tr,
            TEXT,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        SelectObject(dc, ui.font);
        let sep = scale(44, ui.dpi);
        bevel(
            dc,
            &RECT {
                left: scale(16, ui.dpi),
                top: sep,
                right: rc.right - scale(16, ui.dpi),
                bottom: sep + 2,
            },
            false,
        );
        if ui.options {
            for (i, l) in OPTION_LABELS.iter().enumerate() {
                let y = 80 + i as i32 * 34;
                let mut r = RECT {
                    left: scale(16, ui.dpi),
                    top: scale(y, ui.dpi),
                    right: scale(210, ui.dpi),
                    bottom: scale(y + 20, ui.dpi),
                };
                text(dc, l, &mut r, TAN, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
                let h = ui.ctl[&(OPT_BASE + i as i32)];
                let mut wr = RECT::default();
                let _ = GetWindowRect(h, &mut wr);
                let mut pts = [
                    windows::Win32::Foundation::POINT {
                        x: wr.left,
                        y: wr.top,
                    },
                    windows::Win32::Foundation::POINT {
                        x: wr.right,
                        y: wr.bottom,
                    },
                ];
                windows::Win32::Graphics::Gdi::MapWindowPoints(None, hwnd, &mut pts);
                bevel(
                    dc,
                    &RECT {
                        left: pts[0].x - 1,
                        top: pts[0].y - 1,
                        right: pts[1].x + 1,
                        bottom: pts[1].y + 1,
                    },
                    false,
                );
            }
            let mut r = RECT {
                left: scale(16, ui.dpi),
                top: scale(80 + 12 * 34, ui.dpi),
                right: rc.right - 16,
                bottom: scale(80 + 12 * 34 + 60, ui.dpi),
            };
            let addr = format!(
                "Whisper runs on the CPU through whisper.cpp; point the fields above at whisper-cli.exe and a ggml model file.\nThe feed server is listening on {}. Use 0.0.0.0 to reach it from your phone on the same network, or 127.0.0.1 for this PC only.",
                ui.server_addr
            );
            text(dc, &addr, &mut r, DIM, DT_LEFT);
        } else {
            for f in &ui.frames {
                bevel(dc, f, false);
            }
            for (r, s, tan) in &ui.labels {
                let mut r = *r;
                text(
                    dc,
                    s,
                    &mut r,
                    if *tan { TAN } else { DIM },
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                );
            }
        }
        SelectObject(dc, old);
    });
    let _ = EndPaint(hwnd, &ps);
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
    {
        FillRect(
            dc,
            &r,
            HBRUSH(BRUSH_BG.load(std::sync::atomic::Ordering::Relaxed) as _),
        );
        bevel(dc, &r, !pressed);
        let old = SelectObject(dc, font());
        let n = GetWindowTextLengthW(dis.hwndItem);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(dis.hwndItem, &mut buf);
        let label = String::from_utf16_lossy(&buf[..got as usize]);
        let mut tr = r;
        if pressed {
            tr.left += 2;
            tr.top += 2;
        }
        let is_check = dis.CtlID as i32 == B_TOKEN;
        if is_check {
            tr.left += 8;
        }
        text(
            dc,
            &label,
            &mut tr,
            if disabled { DIM } else { TEXT },
            if is_check { DT_LEFT } else { DT_CENTER } | DT_SINGLELINE | DT_VCENTER,
        );
        SelectObject(dc, old);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
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
            SetTextColor(dc, COLORREF(if is_status { TAN } else { TEXT }));
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
                with(|ui| ui.busy = false);
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
