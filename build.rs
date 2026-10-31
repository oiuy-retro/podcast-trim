use std::fs;
use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon = out.join("app.ico");
    fs::write(&icon, make_ico()).unwrap();
    let manifest = out.join("app.manifest");
    fs::write(&manifest, MANIFEST).unwrap();

    let mut res = winres::WindowsResource::new();
    res.set_icon(icon.to_str().unwrap());
    res.set_manifest_file(manifest.to_str().unwrap());
    res.set("FileDescription", "Podcast Trim");
    res.set("ProductName", "Podcast Trim");
    res.set("OriginalFilename", "Podcast Trim.exe");
    if !cfg!(windows) {
        res.set_windres_path("x86_64-w64-mingw32-windres");
        res.set_ar_path("x86_64-w64-mingw32-ar");
    }
    res.compile().expect("embed the icon and manifest");
    // GNU ld drops the icon unless the resource archive is pulled in whole.
    if std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "gnu" {
        let lib = out.join("libresource.a");
        println!("cargo:rustc-link-arg-bins=-Wl,--whole-archive");
        println!("cargo:rustc-link-arg-bins={}", lib.display());
        println!("cargo:rustc-link-arg-bins=-Wl,--no-whole-archive");
    }
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="0.1.0.0" name="Podcast.Trim" type="win32"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}"/>
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn make_ico() -> Vec<u8> {
    let images = [
        (16u32, paint(16)),
        (32, paint(32)),
        (48, paint(48)),
        (256, paint(256)),
    ];
    let mut file = Vec::new();
    file.extend_from_slice(&0u16.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + images.len() * 16;
    let mut blobs = Vec::new();
    for (size, pixels) in &images {
        let size = *size;
        let xor = size * size * 4;
        let mask_stride = size.div_ceil(32) * 4;
        let mask = mask_stride * size;
        let bytes = 40 + xor + mask;
        file.push(if size == 256 { 0 } else { size as u8 });
        file.push(if size == 256 { 0 } else { size as u8 });
        file.push(0);
        file.push(0);
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&bytes.to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += bytes as usize;
        blobs.push(dib(pixels, size));
    }
    for blob in blobs {
        file.extend_from_slice(&blob);
    }
    file
}

fn dib(pixels: &[u8], size: u32) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&40u32.to_le_bytes());
    blob.extend_from_slice(&(size as i32).to_le_bytes());
    blob.extend_from_slice(&((size as i32) * 2).to_le_bytes());
    blob.extend_from_slice(&1u16.to_le_bytes());
    blob.extend_from_slice(&32u16.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    let stride = size as usize;
    for y in (0..stride).rev() {
        let start = y * stride * 4;
        blob.extend_from_slice(&pixels[start..start + stride * 4]);
    }
    let mask_stride = size.div_ceil(32) * 4;
    for _ in 0..size {
        blob.extend(std::iter::repeat_n(0, mask_stride as usize));
    }
    blob
}

fn paint(size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let mut put = |x: i32, y: i32, r: u8, g: u8, b: u8, a: u8| {
        if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
            return;
        }
        let i = (y as usize * size + x as usize) * 4;
        px[i] = b;
        px[i + 1] = g;
        px[i + 2] = r;
        px[i + 3] = a;
    };
    let s = size as i32;
    for y in 0..s {
        for x in 0..s {
            put(x, y, 0, 0, 0, 0);
        }
    }
    let margin = (s / 16).max(1);
    let left = margin;
    let top = margin;
    let right = s - margin;
    let bottom = s - margin;
    // Olive VGUI-style panel with a khaki title strip.
    for y in top..bottom {
        for x in left..right {
            put(x, y, 76, 88, 68, 255);
        }
    }
    let title = (s / 5).max(3);
    for y in top..(top + title).min(bottom) {
        let t = (y - top) as f32 / title.max(1) as f32;
        let r = (196.0 - t * 20.0) as u8;
        let g = (181.0 - t * 20.0) as u8;
        let b = (80.0 - t * 10.0) as u8;
        for x in left..right {
            put(x, y, r, g, b, 255);
        }
    }
    for x in left..right {
        put(x, top, 136, 145, 128, 255);
        put(x, bottom - 1, 40, 46, 34, 255);
    }
    for y in top..bottom {
        put(left, y, 136, 145, 128, 255);
        put(right - 1, y, 40, 46, 34, 255);
    }
    // Waveform in the body.
    let mid = top + title + (bottom - top - title) / 2;
    let wave_left = left + (s / 8).max(2);
    let wave_right = left + (s * 5 / 8).max(wave_left + 4);
    for x in wave_left..wave_right {
        let t = (x - wave_left) as f32 / (wave_right - wave_left).max(1) as f32;
        let amp = ((t * std::f32::consts::TAU * 2.0).sin().abs() * (s / 6) as f32) as i32 + 1;
        for y in (mid - amp)..=(mid + amp) {
            put(x, y, 216, 222, 211, 255);
        }
    }
    // Green convert arrow, readable at 16px.
    let ax = wave_right + (s / 16).max(1);
    let aw = (right - ax - 2).max(3);
    let ah = (s / 3).max(4);
    let ay = mid - ah / 2;
    let shaft_h = (ah / 3).max(2);
    let head_w = (aw / 3).max(2);
    for y in (mid - shaft_h / 2)..(mid + shaft_h / 2 + 1) {
        for x in ax..(ax + aw - head_w) {
            put(x, y, 196, 181, 80, 255);
            if y == mid - shaft_h / 2 {
                put(x, y, 240, 230, 160, 255);
            }
        }
    }
    for i in 0..head_w {
        let y0 = ay + i * ah / head_w.max(1) / 2;
        let y1 = ay + ah - i * ah / head_w.max(1) / 2;
        let x = ax + aw - head_w + i;
        for y in y0..y1 {
            put(x, y, 196, 181, 80, 255);
            if y < y0 + (y1 - y0) / 3 {
                put(x, y, 240, 230, 160, 255);
            }
        }
    }
    px
}
