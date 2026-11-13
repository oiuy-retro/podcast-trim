//! Main window size, position and maximized state, kept in the `[window]`
//! section of configuration.conf, plus the pure layout math for resizing.
use crate::config::Conf;

/// A window rect in screen pixels: left, top, width, height.
pub type Rect = (i32, i32, i32, i32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinState {
    pub rect: Rect,
    pub maximized: bool,
}

const KEYS: [&str; 4] = ["x", "y", "width", "height"];

impl WinState {
    /// Read `[window]`; `None` unless all four numbers are there and sane.
    pub fn from_conf(conf: &Conf) -> Option<WinState> {
        let mut v = [0i32; 4];
        for (i, k) in KEYS.iter().enumerate() {
            v[i] = conf.get("window", k)?.trim().parse().ok()?;
        }
        if v[2] <= 0 || v[3] <= 0 {
            return None;
        }
        let maximized = conf
            .get("window", "maximized")
            .is_some_and(|m| matches!(m.trim().to_ascii_lowercase().as_str(), "yes" | "on" | "true" | "1"));
        Some(WinState {
            rect: (v[0], v[1], v[2], v[3]),
            maximized,
        })
    }

    /// Write `[window]` (one line per key; other lines stay as they are).
    pub fn to_conf(&self, conf: &mut Conf) {
        let (x, y, w, h) = self.rect;
        for (k, v) in KEYS.iter().zip([x, y, w, h]) {
            conf.set("window", k, &v.to_string());
        }
        conf.set("window", "maximized", if self.maximized { "yes" } else { "no" });
    }
}

/// Fit a saved window rect onto a monitor work area (left, top, right,
/// bottom): at least `min` and at most the work area in size, and moved fully
/// onto it, so a window saved on a monitor that is gone comes back visible.
pub fn fit(r: Rect, work: (i32, i32, i32, i32), min: (i32, i32)) -> Rect {
    let (ww, wh) = ((work.2 - work.0).max(1), (work.3 - work.1).max(1));
    let w = r.2.max(min.0).min(ww.max(min.0));
    let h = r.3.max(min.1).min(wh.max(min.1));
    let x = r.0.min(work.2 - w).max(work.0);
    let y = r.1.min(work.3 - h).max(work.1);
    (x, y, w, h)
}

/// Centre a `size` window on a work area (first start, or nothing saved).
pub fn centred(size: (i32, i32), work: (i32, i32, i32, i32)) -> Rect {
    let x = work.0 + ((work.2 - work.0) - size.0) / 2;
    let y = work.1 + ((work.3 - work.1) - size.1) / 2;
    (x.max(work.0), y.max(work.1), size.0, size.1)
}

/// Move a design-size rect for a window `dx`/`dy` larger than the design
/// size. `a` gives the fraction of the extra width added to the left and
/// right edge and of the extra height added to the top and bottom edge.
pub fn anchored(r: Rect, a: (f32, f32, f32, f32), dx: i32, dy: i32) -> Rect {
    let f = |v: f32, d: i32| (v * d as f32).round() as i32;
    let x0 = r.0 + f(a.0, dx);
    let x1 = r.0 + r.2 + f(a.1, dx);
    let y0 = r.1 + f(a.2, dy);
    let y1 = r.1 + r.3 + f(a.3, dy);
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_through_conf() {
        let mut c = Conf::parse("[tools]\nffmpeg = auto\n");
        let s = WinState { rect: (-1200, 40, 1300, 820), maximized: true };
        s.to_conf(&mut c);
        assert!(c.text().contains("[window]\r\nx = -1200\r\ny = 40\r\nwidth = 1300\r\nheight = 820\r\nmaximized = yes"));
        let back = WinState::from_conf(&Conf::parse(&c.text())).unwrap();
        assert_eq!(back, s);
        // Saving again rewrites the same lines, not new ones.
        WinState { rect: (1, 2, 900, 600), maximized: false }.to_conf(&mut c);
        assert_eq!(c.text().matches("width").count(), 1);
        assert!(c.text().contains("maximized = no"));
        assert_eq!(c.text().matches("[tools]").count(), 1);
    }

    #[test]
    fn missing_or_bad_values_are_ignored() {
        assert_eq!(WinState::from_conf(&Conf::parse("")), None);
        assert_eq!(WinState::from_conf(&Conf::parse("[window]\nx = 1\ny = 2\nwidth = 3\n")), None);
        assert_eq!(WinState::from_conf(&Conf::parse("[window]\nx=a\ny=2\nwidth=3\nheight=4")), None);
        assert_eq!(WinState::from_conf(&Conf::parse("[window]\nx=1\ny=2\nwidth=0\nheight=4")), None);
        let s = WinState::from_conf(&Conf::parse("[window]\nx=1\ny=2\nwidth=3\nheight=4")).unwrap();
        assert!(!s.maximized);
    }

    #[test]
    fn fit_keeps_windows_on_screen() {
        let work = (0, 0, 1920, 1040);
        // Already fine: unchanged.
        assert_eq!(fit((100, 50, 1200, 800), work, (900, 600)), (100, 50, 1200, 800));
        // On a monitor that is gone (far left / far right / below).
        assert_eq!(fit((-2500, 100, 1200, 800), work, (900, 600)), (0, 100, 1200, 800));
        assert_eq!(fit((5000, 100, 1200, 800), work, (900, 600)), (720, 100, 1200, 800));
        assert_eq!(fit((10, 3000, 1200, 800), work, (900, 600)), (10, 240, 1200, 800));
        // Too small grows to the minimum; too big shrinks to the work area.
        assert_eq!(fit((10, 10, 300, 200), work, (900, 600)), (10, 10, 900, 600));
        assert_eq!(fit((-5, -5, 4000, 3000), work, (900, 600)), (0, 0, 1920, 1040));
        // A work area smaller than the minimum keeps the minimum at its top left.
        assert_eq!(fit((50, 50, 900, 600), (0, 0, 800, 500), (900, 600)), (0, 0, 900, 600));
        // Second monitor to the left with a negative origin.
        assert_eq!(fit((-1000, 0, 900, 600), (-1920, 0, 0, 1080), (900, 600)), (-1000, 0, 900, 600));
    }

    #[test]
    fn centred_on_work_area() {
        assert_eq!(centred((900, 600), (0, 0, 1920, 1080)), (510, 240, 900, 600));
        assert_eq!(centred((900, 600), (1920, 0, 3840, 1040)), (2430, 220, 900, 600));
    }

    #[test]
    fn anchors_stretch_and_pin() {
        // Pinned to the bottom right: moves, keeps its size.
        assert_eq!(anchored((766, 539, 110, 24), (1.0, 1.0, 1.0, 1.0), 300, 200), (1066, 739, 110, 24));
        // Stretches fully: grows by the whole extra size.
        assert_eq!(anchored((24, 516, 852, 16), (0.0, 1.0, 1.0, 1.0), 300, 200), (24, 716, 1152, 16));
        // Middle column of three (0.3..0.65 of the extra width).
        assert_eq!(anchored((300, 68, 296, 296), (0.3, 0.65, 0.0, 1.0), 100, 100), (330, 68, 331, 396));
        // At the design size nothing moves.
        assert_eq!(anchored((1, 2, 3, 4), (0.3, 0.65, 0.0, 1.0), 0, 0), (1, 2, 3, 4));
    }

    /// The main page's columns at several sizes never overlap: shows list
    /// ends before the episode list, which ends before Details.
    #[test]
    fn columns_never_overlap() {
        let cols = [
            ((24, 68, 264, 296), (0.0, 0.3, 0.0, 1.0)),
            ((300, 68, 296, 296), (0.3, 0.65, 0.0, 1.0)),
            ((608, 68, 268, 196), (0.65, 1.0, 0.0, 1.0)),
        ];
        for dx in [0, 1, 7, 300, 999, 2500] {
            let r: Vec<_> = cols.iter().map(|(r, a)| anchored(*r, *a, dx, 0)).collect();
            assert!(r[0].0 + r[0].2 < r[1].0, "dx {dx}: {r:?}");
            assert!(r[1].0 + r[1].2 < r[2].0, "dx {dx}: {r:?}");
            assert_eq!(r[2].0 + r[2].2, 876 + dx);
        }
    }
}
