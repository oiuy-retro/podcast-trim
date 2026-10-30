#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod net;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    std::process::exit(ui::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Podcast Trim is a Windows program. Build it for x86_64-pc-windows-gnu.");
    std::process::exit(1);
}
