//! "Bulk prompt..." dialog: tick several transcribed episodes and copy one
//! AI prompt for all of them. Drawn like the main window (olive panel,
//! bevelled list with a themed scroll bar, flat buttons) and resizable.

use super::*;

const P_LIST: i32 = 400;
const P_COPY: i32 = 401;
const P_CANCEL: i32 = 402;
/// Sent by IsDialogMessage for Enter and Esc.
const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;
/// Base and smallest size in 96-DPI pixels.
const PW: i32 = 560;
const PH: i32 = 400;
const MIN_W: i32 = 420;
const MIN_H: i32 = 280;
const LIST_TOP: i32 = 58;
const BOTTOM: i32 = 46;

struct Picker {
    hwnd: HWND,
    list: HWND,
    clip: HWND,
    sb: HWND,
    close: HWND,
    copy: HWND,
    cancel: HWND,
    /// (show id, guid) per list row.
    items: Vec<(u32, String)>,
    max: usize,
    note: String,
}

thread_local! {
    static PICK: RefCell<Option<Picker>> = const { RefCell::new(None) };
}

/// The open picker, for the message loop's IsDialogMessage.
pub fn current() -> Option<HWND> {
    PICK.with(|p| p.borrow().as_ref().map(|p| p.hwnd))
}

fn dpi() -> i32 {
    DPI.load(std::sync::atomic::Ordering::Relaxed)
}

fn main_hwnd() -> HWND {
    HWND(MAIN.load(std::sync::atomic::Ordering::Relaxed) as _)
}

/// Every transcribed episode that has an audio hash: (show, guid, label),
/// and the row of the episode selected in the main window, if listed.
fn gather() -> (Vec<(u32, String, String)>, Option<usize>, usize) {
    with(|ui| {
        let (show, guid) = super::current(ui);
        let sh = ui.shared.lock().unwrap();
        let mut rows = Vec::new();
        let mut pre = None;
        for s in &sh.lib.shows {
            for e in &s.episodes {
                if e.state == State::Transcribed && e.record.short_hash().is_some() {
                    if Some(s.id) == show && Some(&e.guid) == guid.as_ref() {
                        pre = Some(rows.len());
                    }
                    let name = if s.title.is_empty() { &s.feed_url } else { &s.title };
                    rows.push((s.id, e.guid.clone(), format!("{}   ({name})", e.title)));
                }
            }
        }
        (rows, pre, sh.conf.bulk_size())
    })
    .unwrap_or_default()
}

pub unsafe fn open() {
    if let Some(h) = current() {
        let _ = SetForegroundWindow(h);
        return;
    }
    let (rows, pre, max) = gather();
    if rows.is_empty() {
        return;
    }
    let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(picker_proc),
        hInstance: instance,
        lpszClassName: w!("PodcastTrimPicker"),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    });
    let d = dpi();
    let main = main_hwnd();
    let mut mr = RECT::default();
    let _ = GetWindowRect(main, &mut mr);
    let (w, h) = (scale(PW, d), scale(PH, d));
    let x = (mr.left + mr.right - w) / 2;
    let y = (mr.top + mr.bottom - h) / 2;
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        w!("PodcastTrimPicker"),
        w!("Bulk AI prompt"),
        WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_CLIPCHILDREN,
        x,
        y,
        w,
        h,
        main,
        None,
        instance,
        None,
    ) else {
        return;
    };
    let font = font();
    let mk = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, parent: HWND, id: i32| {
        let t = wide(text);
        let c = CreateWindowExW(
            ex,
            class,
            PCWSTR(t.as_ptr()),
            style | WS_CHILD | WS_VISIBLE,
            0,
            0,
            10,
            10,
            parent,
            HMENU(id as _),
            instance,
            None,
        )
        .unwrap_or_default();
        SendMessageW(c, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        c
    };
    let close = mk(w!("BUTTON"), "", WINDOW_STYLE(BS_OWNERDRAW as u32), WINDOW_EX_STYLE(0), hwnd, B_CLOSE);
    let clip = mk(w!("PodcastTrimClip"), "", WS_CLIPCHILDREN, WS_EX_CONTROLPARENT, hwnd, 0);
    let list_style = LBS_NOTIFY as u32
        | LBS_MULTIPLESEL as u32
        | LBS_OWNERDRAWFIXED as u32
        | LBS_HASSTRINGS as u32
        | LBS_NOINTEGRALHEIGHT as u32
        | LBS_DISABLENOSCROLL as u32
        | WS_VSCROLL.0
        | WS_TABSTOP.0;
    let list = mk(w!("LISTBOX"), "", WINDOW_STYLE(list_style), WINDOW_EX_STYLE(0), clip, P_LIST);
    let sb = mk(w!("PodcastTrimScroll"), "", WINDOW_STYLE(0), WINDOW_EX_STYLE(0), hwnd, 0);
    SetWindowLongPtrW(sb, GWLP_USERDATA, list.0 as isize);
    SetTimer(sb, 1, 120, None);
    let copy = mk(w!("BUTTON"), "Copy prompt", WINDOW_STYLE(BTN), WINDOW_EX_STYLE(0), hwnd, P_COPY);
    let cancel = mk(w!("BUTTON"), "Cancel", WINDOW_STYLE(BTN), WINDOW_EX_STYLE(0), hwnd, P_CANCEL);
    for (_, _, label) in &rows {
        let w = wide(label);
        SendMessageW(list, LB_ADDSTRING, WPARAM(0), LPARAM(w.as_ptr() as isize));
    }
    if let Some(i) = pre {
        SendMessageW(list, LB_SETSEL, WPARAM(1), LPARAM(i as isize));
        SendMessageW(list, LB_SETCARETINDEX, WPARAM(i), LPARAM(0));
    }
    PICK.with(|p| {
        *p.borrow_mut() = Some(Picker {
            hwnd,
            list,
            clip,
            sb,
            close,
            copy,
            cancel,
            items: rows.into_iter().map(|(s, g, _)| (s, g)).collect(),
            max,
            note: String::new(),
        })
    });
    layout(hwnd);
    update(hwnd);
    let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(main, false);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(list);
}

/// Client size in 96-DPI pixels.
unsafe fn size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let d = dpi();
    (rc.right * 96 / d, rc.bottom * 96 / d)
}

/// The list's sunken frame, in 96-DPI pixels.
fn frame(w: i32, h: i32) -> (i32, i32, i32, i32) {
    (14, LIST_TOP, w - 28, h - LIST_TOP - BOTTOM)
}

unsafe fn layout(hwnd: HWND) {
    let d = dpi();
    let (w, h) = size(hwnd);
    let place = |c: HWND, x: i32, y: i32, cw: i32, ch: i32| {
        let _ = SetWindowPos(c, None, scale(x, d), scale(y, d), scale(cw, d), scale(ch, d), SWP_NOZORDER | SWP_NOACTIVATE);
    };
    PICK.with(|p| {
        let Some(p) = p.borrow().as_ref().map(|p| (p.close, p.clip, p.list, p.sb, p.copy, p.cancel)) else {
            return;
        };
        let (close, clip, list, sb, copy, cancel) = p;
        place(close, w - 26, 5, 18, 17);
        let (fx, fy, fw, fh) = frame(w, h);
        let (ix, iy, iw, ih) = (fx + 1, fy + 1, fw - 2, fh - 2);
        let hh = 20;
        place(clip, ix, iy + hh, iw - SB_W, ih - hh);
        let _ = SetWindowPos(
            list,
            None,
            0,
            0,
            scale(iw - SB_W, d) + GetSystemMetrics(SM_CXVSCROLL),
            scale(ih - hh, d),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        place(sb, ix + iw - SB_W, iy, SB_W, ih);
        place(copy, w - 14 - 80 - 6 - 110, h - 36, 110, 24);
        place(cancel, w - 14 - 80, h - 36, 80, 24);
    });
    let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
}

fn ticked(list: HWND) -> Vec<usize> {
    unsafe {
        let n = SendMessageW(list, LB_GETSELCOUNT, WPARAM(0), LPARAM(0)).0.max(0) as usize;
        let mut idx = vec![0i32; n];
        let got = SendMessageW(list, LB_GETSELITEMS, WPARAM(n), LPARAM(idx.as_mut_ptr() as isize)).0.max(0) as usize;
        idx[..got.min(n)].iter().map(|i| *i as usize).collect()
    }
}

/// Enforce the limit after a click, then refresh the count and Copy.
unsafe fn update(hwnd: HWND) {
    PICK.with(|p| {
        let mut g = p.borrow_mut();
        let Some(p) = g.as_mut() else { return };
        let mut n = ticked(p.list).len();
        if n > p.max {
            let caret = SendMessageW(p.list, LB_GETCARETINDEX, WPARAM(0), LPARAM(0)).0;
            if caret >= 0 {
                SendMessageW(p.list, LB_SETSEL, WPARAM(0), LPARAM(caret));
            }
            n = ticked(p.list).len();
            p.note = "the most per prompt (see Options)".into();
        } else {
            p.note.clear();
        }
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(p.copy, n > 0);
        let _ = InvalidateRect(p.copy, None, false);
    });
    let _ = InvalidateRect(hwnd, None, false);
}

/// Build the combined prompt for the ticked rows and put it on the clipboard.
unsafe fn copy(hwnd: HWND) {
    let Some((rows, list)) = PICK.with(|p| p.borrow().as_ref().map(|p| (p.items.clone(), p.list))) else {
        return;
    };
    let picked: Vec<(u32, String)> = ticked(list).into_iter().filter_map(|i| rows.get(i).cloned()).collect();
    if picked.is_empty() {
        return;
    }
    let text = with(|ui| {
        let sh = ui.shared.lock().unwrap();
        let mut items = Vec::new();
        for (sid, guid) in &picked {
            let Some(s) = sh.lib.show(*sid) else { continue };
            let Some(e) = s.episodes.iter().find(|e| &e.guid == guid) else { continue };
            let Some(hash) = e.record.short_hash() else { continue };
            items.push(prompt::Item {
                show: &s.title,
                title: &e.title,
                duration: format_time(e.record.duration),
                hash,
                transcript: &e.record.transcript,
            });
        }
        (items.len(), prompt::build_bulk(&items))
    });
    let Some((n, text)) = text else { return };
    let ok = n > 0 && copy_to_clipboard(hwnd, &text);
    let _ = DestroyWindow(hwnd);
    with(|ui| {
        status(
            ui,
            &if ok {
                format!(
                    "Prompt for {} copied. Paste it into your AI, then paste its whole reply in the box on the right and press Check cuts.",
                    store::plural(n, "episode")
                )
            } else {
                "Could not open the clipboard.".to_string()
            },
        )
    });
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let d = dpi();
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = size(hwnd);
    let r = |x: i32, y: i32, cw: i32, ch: i32| RECT {
        left: scale(x, d),
        top: scale(y, d),
        right: scale(x + cw, d),
        bottom: scale(y + ch, d),
    };
    fill(dc, &rc, BG);
    bevel(dc, &rc, true);
    let old = SelectObject(dc, font());
    let flags = DT_LEFT | DT_SINGLELINE | DT_VCENTER | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS;
    text(dc, "Copy one AI prompt for several episodes", &mut r(10, 2, w - 50, CAPTION_H - 2), TEXT, flags);
    let (max, note, n) = PICK.with(|p| {
        p.borrow()
            .as_ref()
            .map(|p| (p.max, p.note.clone(), ticked(p.list).len()))
            .unwrap_or_default()
    });
    text(
        dc,
        &format!(
            "Tick up to {} transcribed episodes. The AI answers with each episode's cuts under its hash.",
            max
        ),
        &mut r(14, 32, w - 28, 20),
        TEXT,
        flags,
    );
    let (fx, fy, fw, fh) = frame(w, h);
    let fr = r(fx, fy, fw, fh);
    fill(dc, &RECT { left: fr.left + 1, top: fr.top + 1, right: fr.right - 1, bottom: fr.bottom - 1 }, FIELD);
    bevel(dc, &fr, false);
    let hdr = r(fx + 1, fy + 1, fw - 2 - SB_W, 20);
    fill(dc, &hdr, BG);
    bevel(dc, &hdr, true);
    let mut ht = hdr;
    ht.left += scale(5, d);
    text(dc, "Episode", &mut ht, TEXT, flags);
    let count = if note.is_empty() {
        format!("{n} of {max} ticked")
    } else {
        format!("{n} of {max} ticked: {note}")
    };
    text(dc, &count, &mut r(14, h - 36, w - 14 - 230, 24), if note.is_empty() { TEXT } else { TAN }, flags);
    grip(dc, &rc);
    SelectObject(dc, old);
    let _ = EndPaint(hwnd, &ps);
}

/// A list row: a VGUI check box and the episode, khaki when ticked.
unsafe fn draw_row(dis: &DRAWITEMSTRUCT) {
    if dis.itemID == u32::MAX {
        return;
    }
    let dc = dis.hDC;
    let r = dis.rcItem;
    let d = dpi();
    let on = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let focus = (dis.itemState.0 & ODS_FOCUS.0) != 0;
    fill(dc, &r, FIELD);
    let s = scale(13, d);
    let bx = RECT {
        left: r.left + scale(4, d),
        top: (r.top + r.bottom - s) / 2,
        right: r.left + scale(4, d) + s,
        bottom: (r.top + r.bottom + s) / 2,
    };
    fill(dc, &bx, FIELD);
    bevel(dc, &bx, false);
    let pen = CreatePen(PS_SOLID, 1, COLORREF(TEXT));
    let o = SelectObject(dc, pen);
    if on {
        for k in 0..2 {
            line(dc, bx.left + 3, bx.top + 5 + k, bx.left + 5, bx.top + 7 + k);
            line(dc, bx.left + 5, bx.top + 7 + k, bx.left + 10, bx.top + 2 + k);
        }
    }
    SelectObject(dc, o);
    let _ = DeleteObject(pen);
    if focus {
        let p = CreatePen(PS_SOLID, 1, COLORREF(LIGHT));
        let o = SelectObject(dc, p);
        line(dc, r.left, r.top, r.right - 1, r.top);
        line(dc, r.right - 1, r.top, r.right - 1, r.bottom - 1);
        line(dc, r.right - 1, r.bottom - 1, r.left, r.bottom - 1);
        line(dc, r.left, r.bottom - 1, r.left, r.top);
        SelectObject(dc, o);
        let _ = DeleteObject(p);
    }
    let n = SendMessageW(dis.hwndItem, LB_GETTEXTLEN, WPARAM(dis.itemID as usize), LPARAM(0)).0.max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    SendMessageW(dis.hwndItem, LB_GETTEXT, WPARAM(dis.itemID as usize), LPARAM(buf.as_mut_ptr() as isize));
    let label = String::from_utf16_lossy(&buf[..n]);
    let old = SelectObject(dc, font());
    let mut tr = r;
    tr.left = bx.right + scale(7, d);
    text(
        dc,
        &label,
        &mut tr,
        if on { TAN } else { TEXT },
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS,
    );
    SelectObject(dc, old);
}

unsafe extern "system" fn picker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCCALCSIZE => LRESULT(0),
        WM_NCACTIVATE => LRESULT(1),
        WM_NCHITTEST => {
            let mut pt = windows::Win32::Foundation::POINT {
                x: (lp.0 & 0xffff) as i16 as i32,
                y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
            };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            let d = dpi();
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let hit = edge_hit(pt.x, pt.y, rc.right, rc.bottom, scale(5, d), scale(16, d));
            if hit != 0 {
                return LRESULT(hit as isize);
            }
            if pt.y >= 0 && pt.y < scale(CAPTION_H, d) {
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_GETMINMAXINFO => {
            let d = dpi();
            let mm = &mut *(lp.0 as *mut MINMAXINFO);
            mm.ptMinTrackSize.x = scale(MIN_W, d);
            mm.ptMinTrackSize.y = scale(MIN_H, d);
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd);
            LRESULT(0)
        }
        WM_MEASUREITEM => {
            let mis = &mut *(lp.0 as *mut windows::Win32::UI::Controls::MEASUREITEMSTRUCT);
            mis.itemHeight = scale(20, dpi()) as u32;
            LRESULT(1)
        }
        WM_DRAWITEM => {
            let dis = &*(lp.0 as *const DRAWITEMSTRUCT);
            if dis.CtlType == windows::Win32::UI::Controls::ODT_LISTBOX {
                draw_row(dis);
            } else {
                draw_button(dis);
            }
            LRESULT(1)
        }
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let dc = HDC(wp.0 as _);
            SetTextColor(dc, COLORREF(TEXT));
            SetBkColor(dc, COLORREF(FIELD));
            LRESULT(BRUSH_FIELD.load(std::sync::atomic::Ordering::Relaxed))
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let code = ((wp.0 >> 16) & 0xffff) as u32;
            match id {
                P_LIST if code == LBN_SELCHANGE => update(hwnd),
                P_COPY | IDOK if code == BN_CLICKED => copy(hwnd),
                P_CANCEL | B_CLOSE | IDCANCEL if code == BN_CLICKED => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PICK.with(|p| *p.borrow_mut() = None);
            let main = main_hwnd();
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(main, true);
            let _ = SetForegroundWindow(main);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
