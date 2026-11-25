//! Show and episode cover artwork: download, badge with "TRIMMED" in the
//! system Impact font (Option A proportions), cache per show, and resolve
//! local URLs for the feed server.

use crate::store::{sha256_hex, Paths, Show, State};
use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, RgbImage};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Bump when the badge design changes so cached covers rebuild.
pub const STYLE_VERSION: &str = "trimmed-impact-a-v1";
pub const BADGE_TEXT: &str = "TRIMMED";
/// Cap downloaded image size to guard against malformed feeds.
const MAX_ART_BYTES: usize = 16 * 1024 * 1024;
/// Reject absurdly large dimensions before allocating raster buffers.
const MAX_DIMENSION: u32 = 12_000;
const JPEG_QUALITY: u8 = 92;

/// Layout of the Option A "TRIMMED" badge on an image of size `img_w x img_h`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeGeometry {
    pub px_scale: f32,
    pub ink_min_x: f32,
    pub ink_min_y: f32,
    pub ink_w: f32,
    pub ink_h: f32,
    pub stroke: u32,
    pub pad: i32,
    pub origin_x: f32,
    pub origin_y: f32,
}

/// One cached cover entry in `shows/<id>/art/manifest.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtEntry {
    pub style: String,
    pub source_hash: String,
    pub file: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArtManifest {
    #[serde(default)]
    pub entries: HashMap<String, ArtEntry>,
}

/// Snapshot of a show's artwork URLs collected under the library lock.
#[derive(Clone, Debug)]
pub struct SyncItem {
    pub show_id: u32,
    pub show_image: String,
    pub ep_images: Vec<String>,
}

/// Content-addressed cache key from source image SHA-256 and style version.
pub fn cache_key(source_hash: &str, style_version: &str) -> String {
    sha256_hex(format!("{style_version}:{source_hash}").as_bytes())[..24].to_string()
}

pub fn cache_filename(source_hash: &str, style_version: &str) -> String {
    format!("{}.jpg", cache_key(source_hash, style_version))
}

/// Allow only `<hex>.jpg` / `<hex>.png` filenames from `/art/<show>/<file>`.
pub fn is_safe_cache_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".jpg").or_else(|| name.strip_suffix(".png")) else {
        return false;
    };
    !stem.is_empty() && stem.len() <= 64 && stem.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Locate the system Impact TrueType font without shipping a font file.
pub fn system_impact_font() -> Option<PathBuf> {
    system_impact_candidates().into_iter().find(|p| p.is_file())
}

fn system_impact_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let fonts = windir.join("Fonts");
        out.push(fonts.join("impact.ttf"));
        out.push(fonts.join("Impact.ttf"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let user_fonts = PathBuf::from(local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts");
            out.push(user_fonts.join("impact.ttf"));
            out.push(user_fonts.join("Impact.ttf"));
        }
    } else {
        for dir in [
            "/usr/share/fonts/truetype/msttcorefonts",
            "/usr/share/fonts/TTF",
            "/usr/share/fonts",
        ] {
            let d = Path::new(dir);
            out.push(d.join("impact.ttf"));
            out.push(d.join("Impact.ttf"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            let d = PathBuf::from(home).join(".local/share/fonts");
            out.push(d.join("impact.ttf"));
            out.push(d.join("Impact.ttf"));
        }
    }
    out
}

fn measure_text(font: &FontRef<'_>, px: f32) -> Option<(f32, f32, f32, f32, Vec<ab_glyph::Glyph>)> {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let mut x = 0.0f32;
    let mut prev: Option<GlyphId> = None;
    let mut glyphs = Vec::with_capacity(BADGE_TEXT.len());
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for ch in BADGE_TEXT.chars() {
        let gid = font.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, gid);
        }
        let g = gid.with_scale_and_position(scale, point(x, 0.0));
        if let Some(og) = font.outline_glyph(g.clone()) {
            let b = og.px_bounds();
            min_x = min_x.min(b.min.x);
            min_y = min_y.min(b.min.y);
            max_x = max_x.max(b.max.x);
            max_y = max_y.max(b.max.y);
        }
        glyphs.push(g);
        x += scaled.h_advance(gid);
        prev = Some(gid);
    }

    if !min_x.is_finite() || max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some((min_x, min_y, max_x - min_x, max_y - min_y, glyphs))
}

/// Compute Option A placement: full-width (~90% of cover width) tight to the
/// bottom edge (~4.5% margin under the outline), medium black outline (~7.5%
/// of letter height).
pub fn layout_badge(font: &FontRef<'_>, img_w: u32, img_h: u32) -> Option<BadgeGeometry> {
    if img_w < 32 || img_h < 32 {
        return None;
    }
    let (_, _, ref_w, _, _) = measure_text(font, 100.0)?;
    let target_w = img_w as f32 * 0.90;
    let mut px = 100.0 * (target_w / ref_w);

    // One refinement pass keeps ink width within ~1px of 90% of cover width.
    let (_, _, w1, h1, _) = measure_text(font, px)?;
    px *= target_w / w1;

    // Guard non-square short banners so text + outline + bottom pad fit vertically.
    let pad = (img_h as f32 * 0.045).round().max(1.0);
    let est_h = h1 * (px / (100.0 * (target_w / ref_w)));
    let est_stroke = (est_h * 0.075).round().max((img_w as f32 * 0.006).round()).max(2.0);
    let total_h = est_h + 2.0 * est_stroke + 2.0 * pad;
    let max_h = img_h as f32 * 0.92;
    if total_h > max_h && est_h > 0.0 {
        let avail_fill = (max_h - 2.0 * pad).max(8.0) / 1.15;
        px *= avail_fill / est_h;
    }

    let (ink_min_x, ink_min_y, ink_w, ink_h, _) = measure_text(font, px)?;
    let stroke = (ink_h * 0.075)
        .round()
        .max((img_w as f32 * 0.006).round())
        .max(2.0) as u32;
    let pad_i = pad as i32;
    let origin_x = (img_w as f32 - ink_w) * 0.5 - ink_min_x;
    let origin_y = (img_h as i32 - pad_i - stroke as i32) as f32 - (ink_min_y + ink_h);

    Some(BadgeGeometry {
        px_scale: px,
        ink_min_x,
        ink_min_y,
        ink_w,
        ink_h,
        stroke,
        pad: pad_i,
        origin_x,
        origin_y,
    })
}

/// 1D squared Euclidean distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let fq = f[q] + (q * q) as f32;
        let mut s = (fq - (f[v[k]] + (v[k] * v[k]) as f32)) / (2 * (q - v[k])) as f32;
        while k > 0 && s <= z[k] {
            k -= 1;
            s = (fq - (f[v[k]] + (v[k] * v[k]) as f32)) / (2 * (q - v[k])) as f32;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dx = q as f32 - v[k] as f32;
        *dq = dx * dx + f[v[k]];
    }
}

/// Squared Euclidean distance from any pixel with `mask >= 0.5`.
fn distance_sq_from_mask(mask: &[f32], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e12;
    let mut grid: Vec<f32> = mask
        .iter()
        .map(|&a| if a >= 0.5 { 0.0 } else { INF })
        .collect();
    let max_dim = w.max(h);
    let mut f = vec![0.0f32; max_dim];
    let mut d = vec![0.0f32; max_dim];
    let mut v = vec![0usize; max_dim];
    let mut z = vec![0.0f32; max_dim + 1];

    // Columns
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    // Rows
    for y in 0..h {
        let row = &mut grid[y * w..(y + 1) * w];
        edt_1d(row, &mut d[..w], &mut v, &mut z);
        row.copy_from_slice(&d[..w]);
    }
    grid
}

fn flatten_to_rgb(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (px, dst) in rgba.pixels().zip(out.pixels_mut()) {
        let a = px[3] as f32 / 255.0;
        let inv = 1.0 - a;
        dst[0] = (px[0] as f32 * a + 255.0 * inv).round() as u8;
        dst[1] = (px[1] as f32 * a + 255.0 * inv).round() as u8;
        dst[2] = (px[2] as f32 * a + 255.0 * inv).round() as u8;
    }
    out
}

/// Composite white Impact "TRIMMED" with a black border onto `cover`.
pub fn composite_badge(cover: &DynamicImage, font_bytes: &[u8]) -> Result<RgbImage, String> {
    let font = FontRef::try_from_slice(font_bytes).map_err(|e| format!("bad font: {e}"))?;
    let (w, h) = cover.dimensions();
    let mut rgb = flatten_to_rgb(cover);
    let Some(geom) = layout_badge(&font, w, h) else {
        return Ok(rgb);
    };

    let scale = PxScale::from(geom.px_scale);
    let scaled = font.as_scaled(scale);
    let mut x = geom.origin_x;
    let y = geom.origin_y;
    let mut prev: Option<GlyphId> = None;
    let mut outlined = Vec::with_capacity(BADGE_TEXT.len());
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for ch in BADGE_TEXT.chars() {
        let gid = font.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, gid);
        }
        let g = gid.with_scale_and_position(scale, point(x, y));
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            min_x = min_x.min(b.min.x.floor() as i32);
            min_y = min_y.min(b.min.y.floor() as i32);
            max_x = max_x.max(b.max.x.ceil() as i32);
            max_y = max_y.max(b.max.y.ceil() as i32);
            outlined.push(og);
        }
        x += scaled.h_advance(gid);
        prev = Some(gid);
    }

    if outlined.is_empty() || max_x <= min_x || max_y <= min_y {
        return Ok(rgb);
    }

    let margin = geom.stroke as i32 + 2;
    let x0 = min_x - margin;
    let y0 = min_y - margin;
    let mw = (max_x - min_x + 2 * margin) as usize;
    let mh = (max_y - min_y + 2 * margin) as usize;
    let mut mask = vec![0.0f32; mw * mh];

    for og in &outlined {
        let b = og.px_bounds();
        let gx0 = b.min.x.floor() as i32 - x0;
        let gy0 = b.min.y.floor() as i32 - y0;
        og.draw(|gx, gy, c| {
            let mx = gx0 + gx as i32;
            let my = gy0 + gy as i32;
            if mx >= 0 && my >= 0 && (mx as usize) < mw && (my as usize) < mh {
                let idx = my as usize * mw + mx as usize;
                mask[idx] = (mask[idx] + c).min(1.0);
            }
        });
    }

    composite_mask(&mut rgb, x0, y0, mw, mh, &mask, geom.stroke);
    Ok(rgb)
}

fn composite_mask(
    rgb: &mut RgbImage,
    x0: i32,
    y0: i32,
    mw: usize,
    mh: usize,
    mask: &[f32],
    stroke: u32,
) {
    let dist2 = distance_sq_from_mask(mask, mw, mh);
    let (w, h) = rgb.dimensions();
    let r = stroke as f32;

    for my in 0..mh {
        let iy = y0 + my as i32;
        if iy < 0 || iy >= h as i32 {
            continue;
        }
        for mx in 0..mw {
            let ix = x0 + mx as i32;
            if ix < 0 || ix >= w as i32 {
                continue;
            }
            let idx = my * mw + mx;
            let fill_a = mask[idx];
            let outline_a = if fill_a > 0.0 {
                1.0
            } else {
                (r + 0.5 - dist2[idx].sqrt()).clamp(0.0, 1.0)
            };
            if outline_a <= 0.0 {
                continue;
            }
            let p = rgb.get_pixel_mut(ix as u32, iy as u32);
            // First composite black outline over cover, then white glyph over that.
            let inv_out = 1.0 - outline_a;
            let inv_fill = 1.0 - fill_a;
            let white = 255.0 * fill_a;
            for c in 0..3 {
                let with_outline = p[c] as f32 * inv_out;
                let final_c = with_outline * inv_fill + white;
                p[c] = final_c.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Decode an image from memory, draw the badge, and encode as JPEG.
pub fn badge_bytes(image_bytes: &[u8], font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if image_bytes.len() > MAX_ART_BYTES {
        return Err("cover image is larger than 16 MB".into());
    }
    let img = decode_cover(image_bytes)?;
    let out = composite_badge(&img, font_bytes)?;
    let mut buf = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    enc.encode_image(&out).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn decode_cover(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    let mut reader = reader;
    reader.limits(limits);
    reader.decode().map_err(|e| e.to_string())
}

pub fn load_manifest(paths: &Paths, show_id: u32) -> ArtManifest {
    let Ok(bytes) = fs::read(paths.art_manifest(show_id)) else {
        return ArtManifest::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_manifest(paths: &Paths, show_id: u32, manifest: &ArtManifest) -> Result<(), String> {
    let dir = paths.art_dir(show_id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = paths.art_manifest(show_id);
    let part = dir.join("manifest.json.part");
    let data = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    fs::write(&part, data).map_err(|e| e.to_string())?;
    fs::rename(&part, &path).map_err(|e| e.to_string())
}

/// Look up the cached badged cover filename for `image_url`, if present and
/// built with the current `STYLE_VERSION`.
pub fn cached_file(
    paths: &Paths,
    show_id: u32,
    manifest: &ArtManifest,
    image_url: &str,
) -> Option<String> {
    if image_url.is_empty() {
        return None;
    }
    let entry = manifest.entries.get(image_url)?;
    if entry.style != STYLE_VERSION || !is_safe_cache_file(&entry.file) {
        return None;
    }
    paths
        .art_file(show_id, &entry.file)
        .is_file()
        .then(|| entry.file.clone())
}

/// Collect the show's cover URL plus episode artwork URLs to cache: all
/// `Ready` episodes (always served in the feed) and up to `max_new_ep_art`
/// newest episodes with distinct artwork URLs.
pub fn collect_show_urls(show: &Show, max_new_ep_art: usize) -> SyncItem {
    let mut seen: HashSet<&str> = HashSet::new();
    if !show.image_url.is_empty() {
        seen.insert(show.image_url.as_str());
    }
    let mut ep_images = Vec::new();
    for ep in &show.episodes {
        if ep.state == State::Ready
            && !ep.image_url.is_empty()
            && seen.insert(ep.image_url.as_str())
        {
            ep_images.push(ep.image_url.clone());
        }
    }
    let mut extra = 0usize;
    for ep in &show.episodes {
        if extra >= max_new_ep_art {
            break;
        }
        if !ep.image_url.is_empty() && seen.insert(ep.image_url.as_str()) {
            ep_images.push(ep.image_url.clone());
            extra += 1;
        }
    }
    SyncItem {
        show_id: show.id,
        show_image: show.image_url.clone(),
        ep_images,
    }
}

/// Download and badge the show cover and any episode covers for one show.
/// Skips cleanly if Impact is absent or if a URL has no usable image.
pub fn sync_show_item(
    paths: &Paths,
    item: &SyncItem,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) {
    let Some(font_path) = system_impact_font() else {
        return;
    };
    sync_show_item_with_font(paths, item, Some(&font_path), fetch);
}

/// Testable entry point taking an explicit `font_path`.
pub fn sync_show_item_with_font(
    paths: &Paths,
    item: &SyncItem,
    font_path: Option<&Path>,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) {
    let Some(font_path) = font_path else {
        return;
    };
    let Ok(font_bytes) = fs::read(font_path) else {
        return;
    };
    let mut urls: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    if !item.show_image.is_empty() && seen.insert(item.show_image.as_str()) {
        urls.push(item.show_image.as_str());
    }
    for u in &item.ep_images {
        if !u.is_empty() && seen.insert(u.as_str()) {
            urls.push(u.as_str());
        }
    }
    if urls.is_empty() {
        return;
    }

    let mut manifest = load_manifest(paths, item.show_id);
    let mut changed = false;

    for url in urls {
        if cached_file(paths, item.show_id, &manifest, url).is_some() {
            continue;
        }
        let Ok(raw) = fetch(url) else {
            continue;
        };
        if raw.len() > MAX_ART_BYTES {
            continue;
        }
        let source_hash = sha256_hex(&raw);
        let file = cache_filename(&source_hash, STYLE_VERSION);
        let dest = paths.art_file(item.show_id, &file);
        if !dest.is_file() {
            let Ok(composited) = badge_bytes(&raw, &font_bytes) else {
                continue;
            };
            if fs::create_dir_all(paths.art_dir(item.show_id)).is_err() {
                continue;
            }
            let part = dest.with_extension("jpg.part");
            if fs::write(&part, &composited).is_err() || fs::rename(&part, &dest).is_err() {
                let _ = fs::remove_file(&part);
                continue;
            }
        }
        manifest.entries.insert(
            url.to_string(),
            ArtEntry {
                style: STYLE_VERSION.into(),
                source_hash,
                file,
            },
        );
        changed = true;
    }

    if changed {
        prune_unused(paths, item.show_id, &manifest);
        let _ = save_manifest(paths, item.show_id, &manifest);
    }
}

/// Ensure one episode's artwork is cached (e.g. when it becomes `Ready`).
pub fn ensure_episode_url(
    paths: &Paths,
    show_id: u32,
    image_url: &str,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) {
    if image_url.is_empty() {
        return;
    }
    let item = SyncItem {
        show_id,
        show_image: String::new(),
        ep_images: vec![image_url.to_string()],
    };
    sync_show_item(paths, &item, fetch);
}

/// Sync artwork for several shows using up to `workers` threads.
pub fn sync_shows(
    paths: &Paths,
    items: &[SyncItem],
    workers: usize,
    fetch: &(dyn Fn(&str) -> Result<Vec<u8>, String> + Sync),
    stop: &(dyn Fn() -> bool + Sync),
) {
    let Some(font_path) = system_impact_font() else {
        return;
    };
    sync_shows_with_font(paths, items, workers, Some(&font_path), fetch, stop);
}

pub fn sync_shows_with_font(
    paths: &Paths,
    items: &[SyncItem],
    workers: usize,
    font_path: Option<&Path>,
    fetch: &(dyn Fn(&str) -> Result<Vec<u8>, String> + Sync),
    stop: &(dyn Fn() -> bool + Sync),
) {
    let Some(font_path) = font_path else {
        return;
    };
    if items.is_empty() || !font_path.is_file() {
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..workers.clamp(1, items.len()) {
            s.spawn(|| loop {
                if stop() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(i) else {
                    break;
                };
                sync_show_item_with_font(paths, item, Some(font_path), fetch);
            });
        }
    });
}

fn prune_unused(paths: &Paths, show_id: u32, manifest: &ArtManifest) {
    let keep: HashSet<&str> = manifest
        .entries
        .values()
        .filter(|e| e.style == STYLE_VERSION)
        .map(|e| e.file.as_str())
        .collect();
    let Ok(rd) = fs::read_dir(paths.art_dir(show_id)) else {
        return;
    };
    for ent in rd.flatten() {
        let name = ent.file_name();
        let Some(s) = name.to_str() else {
            continue;
        };
        if (s.ends_with(".jpg") || s.ends_with(".png")) && !keep.contains(s) {
            let _ = fs::remove_file(ent.path());
        }
    }
}

/// Remove unused `AtomicBool` warning if none needed outside tests.
#[allow(dead_code)]
fn _unused(_: &AtomicBool) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Episode, Record};
    use image::{ImageBuffer, Rgb};
    use std::sync::atomic::AtomicUsize;

    fn sample_png(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        let img = ImageBuffer::from_fn(w, h, |_, _| Rgb(color));
        let mut buf = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    /// Minimal valid TrueType font containing glyphs for T, R, I, M, E, D
    /// (enough for geometry and rendering tests when system Impact is absent).
    fn minimal_test_ttf() -> Vec<u8> {
        // Build a valid SFNT with `head`, `hhea`, `maxp`, `hmtx`, `loca`, `glyf`, `cmap`, `name`.
        // One simple rectangular contour per glyph in T,R,I,M,E,D; unitsPerEm = 1000.
        let chars = ['D', 'E', 'I', 'M', 'R', 'T'];
        let num_glyphs: u16 = 1 + chars.len() as u16; // .notdef + 6 chars
        let adv: u16 = 520;

        // Simple 4-point rectangle glyph: (40,0) -> (40,800) -> (480,800) -> (480,0).
        let mut rect_glyph = Vec::new();
        rect_glyph.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
        rect_glyph.extend_from_slice(&40i16.to_be_bytes()); // xMin
        rect_glyph.extend_from_slice(&0i16.to_be_bytes()); // yMin
        rect_glyph.extend_from_slice(&480i16.to_be_bytes()); // xMax
        rect_glyph.extend_from_slice(&800i16.to_be_bytes()); // yMax
        rect_glyph.extend_from_slice(&3u16.to_be_bytes()); // endPtsOfContours[0]
        rect_glyph.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        // flags: on-curve (0x01) for 4 points
        rect_glyph.extend_from_slice(&[1, 1, 1, 1]);
        // x-coordinates (deltas from 0): +40, 0, +440, 0
        rect_glyph.extend_from_slice(&40i16.to_be_bytes());
        rect_glyph.extend_from_slice(&0i16.to_be_bytes());
        rect_glyph.extend_from_slice(&440i16.to_be_bytes());
        rect_glyph.extend_from_slice(&0i16.to_be_bytes());
        // y-coordinates (deltas from 0): 0, +800, 0, -800
        rect_glyph.extend_from_slice(&0i16.to_be_bytes());
        rect_glyph.extend_from_slice(&800i16.to_be_bytes());
        rect_glyph.extend_from_slice(&0i16.to_be_bytes());
        rect_glyph.extend_from_slice(&(-800i16).to_be_bytes());
        while rect_glyph.len() % 4 != 0 {
            rect_glyph.push(0);
        }

        // glyf + loca (short format: offsets / 2)
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for _ in 0..num_glyphs {
            loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());
            glyf.extend_from_slice(&rect_glyph);
        }
        loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());

        // head (54 bytes)
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // version
        head[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // fontRevision
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magic
        head[18..20].copy_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
        head[36..38].copy_from_slice(&40i16.to_be_bytes()); // xMin
        head[38..40].copy_from_slice(&0i16.to_be_bytes()); // yMin
        head[40..42].copy_from_slice(&480i16.to_be_bytes()); // xMax
        head[42..44].copy_from_slice(&800i16.to_be_bytes()); // yMax
        // indexToLocFormat = 0 (short)

        // hhea (36 bytes)
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[4..6].copy_from_slice(&800i16.to_be_bytes()); // ascender
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes()); // descender
        hhea[10..12].copy_from_slice(&adv.to_be_bytes()); // advanceWidthMax
        hhea[34..36].copy_from_slice(&num_glyphs.to_be_bytes()); // numberOfHMetrics

        // maxp (32 bytes)
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&num_glyphs.to_be_bytes());

        // hmtx
        let mut hmtx = Vec::new();
        for _ in 0..num_glyphs {
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&40i16.to_be_bytes()); // lsb
        }

        // cmap format 4 (sorted codepoints D,E,I,M,R,T -> glyph ids 1..=6)
        let seg_count: u16 = chars.len() as u16 + 1;
        let seg_x2 = seg_count * 2;
        let mut cmap_sub = Vec::new();
        cmap_sub.extend_from_slice(&4u16.to_be_bytes()); // format
        let sub_len = 16 + 8 * seg_count;
        cmap_sub.extend_from_slice(&sub_len.to_be_bytes());
        cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // language
        cmap_sub.extend_from_slice(&seg_x2.to_be_bytes());
        cmap_sub.extend_from_slice(&8u16.to_be_bytes()); // searchRange
        cmap_sub.extend_from_slice(&2u16.to_be_bytes()); // entrySelector
        cmap_sub.extend_from_slice(&(seg_x2.saturating_sub(8)).to_be_bytes()); // rangeShift
        for &c in &chars {
            cmap_sub.extend_from_slice(&(c as u16).to_be_bytes()); // endCode
        }
        cmap_sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
        cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for &c in &chars {
            cmap_sub.extend_from_slice(&(c as u16).to_be_bytes()); // startCode
        }
        cmap_sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
        for (i, &c) in chars.iter().enumerate() {
            let gid = (i + 1) as i32;
            let delta = (gid - c as i32) as i16;
            cmap_sub.extend_from_slice(&delta.to_be_bytes()); // idDelta
        }
        cmap_sub.extend_from_slice(&1i16.to_be_bytes());
        for _ in 0..seg_count {
            cmap_sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset
        }

        let mut cmap = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes()); // version
        cmap.extend_from_slice(&1u16.to_be_bytes()); // numTables
        cmap.extend_from_slice(&3u16.to_be_bytes()); // platformID: Windows
        cmap.extend_from_slice(&1u16.to_be_bytes()); // encodingID: Unicode BMP
        cmap.extend_from_slice(&12u32.to_be_bytes()); // offset
        cmap.extend_from_slice(&cmap_sub);

        let name = vec![0u8; 6]; // empty name table

        let tables: [(&[u8; 4], Vec<u8>); 8] = [
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
            (b"name", name),
        ];

        let num_tables = tables.len() as u16;
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&num_tables.to_be_bytes());
        out.extend_from_slice(&128u16.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());

        let mut offset = 12 + tables.len() * 16;
        let mut padded = Vec::new();
        for (tag, mut data) in tables {
            let len = data.len() as u32;
            while data.len() % 4 != 0 {
                data.push(0);
            }
            out.extend_from_slice(tag);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&len.to_be_bytes());
            offset += data.len();
            padded.extend_from_slice(&data);
        }
        out.extend_from_slice(&padded);
        out
    }

    #[test]
    fn badge_geometry_matches_option_a_proportions() {
        let ttf = minimal_test_ttf();
        let font = FontRef::try_from_slice(&ttf).unwrap();
        for size in [600u32, 1400, 3000] {
            let g = layout_badge(&font, size, size).unwrap();
            let width_frac = g.ink_w / size as f32;
            assert!(
                (width_frac - 0.90).abs() < 0.02,
                "size {size}: ink_w={} frac={width_frac}",
                g.ink_w
            );
            let pad_frac = g.pad as f32 / size as f32;
            assert!(
                (pad_frac - 0.045).abs() < 0.005,
                "size {size}: pad={} frac={pad_frac}",
                g.pad
            );
            let stroke_ratio = g.stroke as f32 / g.ink_h;
            assert!(
                (stroke_ratio - 0.075).abs() < 0.02,
                "size {size}: stroke={} ink_h={} ratio={stroke_ratio}",
                g.stroke,
                g.ink_h
            );
            // Bottom of white fill sits pad + stroke above bottom edge.
            let bottom_ink = g.origin_y + g.ink_min_y + g.ink_h;
            assert!(
                (bottom_ink - (size as i32 - g.pad - g.stroke as i32) as f32).abs() < 1.0,
                "bottom_ink={bottom_ink} expected={}",
                size as i32 - g.pad - g.stroke as i32
            );
        }
        assert!(layout_badge(&font, 20, 20).is_none());
        // Short wide image shrinks vertically instead of overflowing the top.
        let wide = layout_badge(&font, 1400, 240).unwrap();
        assert!(wide.origin_y + wide.ink_min_y - wide.stroke as f32 >= 0.0);
    }

    #[test]
    fn composite_draws_white_fill_and_black_border_at_bottom() {
        let ttf = minimal_test_ttf();
        let raw = sample_png(600, 600, [80, 120, 180]);
        let badged = badge_bytes(&raw, &ttf).unwrap();
        let decoded = image::load_from_memory(&badged).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (600, 600));

        // Top half stays the original blue background.
        let top = decoded.get_pixel(300, 100);
        assert!(top[2] > 160 && top[0] < 100, "top={top:?}");

        // Bottom margin below the badge also stays blue.
        let below = decoded.get_pixel(300, 595);
        assert!(below[2] > 160 && below[0] < 100, "below={below:?}");

        // Lower region has both bright white fill and dark black outline pixels.
        let mut whites = 0usize;
        let mut blacks = 0usize;
        for y in 400..585 {
            for x in 20..580 {
                let p = decoded.get_pixel(x, y);
                if p[0] > 235 && p[1] > 235 && p[2] > 235 {
                    whites += 1;
                } else if p[0] < 20 && p[1] < 20 && p[2] < 20 {
                    blacks += 1;
                }
            }
        }
        assert!(whites > 5_000, "whites={whites}");
        assert!(blacks > 1_000, "blacks={blacks}");
    }

    #[test]
    fn cache_key_depends_on_source_hash_and_style() {
        let k1 = cache_key("abc123", STYLE_VERSION);
        let k2 = cache_key("abc124", STYLE_VERSION);
        let k3 = cache_key("abc123", "trimmed-impact-a-v2");
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert!(is_safe_cache_file(&format!("{k1}.jpg")));
        assert!(!is_safe_cache_file("../evil.jpg"));
        assert!(!is_safe_cache_file("bad.exe"));
    }

    #[test]
    fn sync_caches_reuses_matching_urls_and_skips_missing_font() {
        let root = std::env::temp_dir().join(format!("pt-art-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let paths = Paths::new(root.clone());

        let font_path = root.join("test.ttf");
        fs::write(&font_path, minimal_test_ttf()).unwrap();

        let show_png = sample_png(400, 400, [40, 90, 140]);
        let ep_png = sample_png(400, 400, [140, 70, 40]);

        let mut show = Show {
            id: 1,
            title: "S".into(),
            feed_url: "http://x/feed".into(),
            token: "tok".into(),
            episodes: vec![
                Episode {
                    guid: "g1".into(),
                    title: "Same as show".into(),
                    pub_date: String::new(),
                    source_url: "u1".into(),
                    state: State::Ready,
                    record: Record::default(),
                    cleaned_bytes: 0,
                    served: 0,
                    image_url: "http://x/show.png".into(),
                },
                Episode {
                    guid: "g2".into(),
                    title: "Own art".into(),
                    pub_date: String::new(),
                    source_url: "u2".into(),
                    state: State::Ready,
                    record: Record::default(),
                    cleaned_bytes: 0,
                    served: 0,
                    image_url: "http://x/ep2.png".into(),
                },
                Episode {
                    guid: "g3".into(),
                    title: "No art".into(),
                    pub_date: String::new(),
                    source_url: "u3".into(),
                    state: State::Ready,
                    record: Record::default(),
                    cleaned_bytes: 0,
                    served: 0,
                    image_url: String::new(),
                },
            ],
            image_url: "http://x/show.png".into(),
        };

        let calls = AtomicUsize::new(0);
        let fetch = |u: &str| -> Result<Vec<u8>, String> {
            calls.fetch_add(1, Ordering::SeqCst);
            match u {
                "http://x/show.png" | "http://x/show-mirror.png" => Ok(show_png.clone()),
                "http://x/ep2.png" => Ok(ep_png.clone()),
                _ => Err("404".into()),
            }
        };

        let item = collect_show_urls(&show, 10);
        // Matching episode URL was deduplicated against show_image.
        assert_eq!(item.ep_images, vec!["http://x/ep2.png".to_string()]);

        // Missing font skips gracefully and makes zero fetches.
        sync_show_item_with_font(&paths, &item, None, &fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(load_manifest(&paths, 1).entries.is_empty());

        // With font: downloads each unique URL once and writes two cached files.
        sync_show_item_with_font(&paths, &item, Some(&font_path), &fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let man = load_manifest(&paths, 1);
        let f_show = cached_file(&paths, 1, &man, &show.image_url).unwrap();
        let f_ep1 = cached_file(&paths, 1, &man, &show.episodes[0].image_url).unwrap();
        let f_ep2 = cached_file(&paths, 1, &man, &show.episodes[1].image_url).unwrap();
        assert_eq!(f_show, f_ep1);
        assert_ne!(f_show, f_ep2);
        assert!(paths.art_file(1, &f_show).is_file());
        assert!(paths.art_file(1, &f_ep2).is_file());

        // Second sync is a complete cache hit (0 network calls).
        sync_show_item_with_font(&paths, &item, Some(&font_path), &fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // If show cover changes to a new URL with the same bytes, file is reused
        // and the stale manifest entry is kept/new URL resolves to same hash file.
        show.image_url = "http://x/show-mirror.png".into();
        let item2 = collect_show_urls(&show, 10);
        sync_show_item_with_font(&paths, &item2, Some(&font_path), &fetch);
        let man2 = load_manifest(&paths, 1);
        assert_eq!(cached_file(&paths, 1, &man2, "http://x/show-mirror.png").unwrap(), f_show);

        let _ = fs::remove_dir_all(&root);
    }
}
