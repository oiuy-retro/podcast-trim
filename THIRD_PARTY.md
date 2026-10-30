# Third-party software

Podcast Trim links against or runs next to the following projects. This repository contains only the Podcast Trim sources. It does not include FFmpeg, whisper.cpp, or any Whisper model. They must be placed in the same folder as `Podcast Trim.exe`.

Tested with FFmpeg `9.0.2-essentials_build` (Gyan) and whisper.cpp `b5454` (`whisper-bin-x64.zip`) with `ggml-tiny.en.bin`.

## Rust crates (linked into the exe)

| Project | License | Use |
| --- | --- | --- |
| [windows-rs](https://github.com/microsoft/windows-rs) | MIT OR Apache-2.0 | Win32 UI, GDI drawing, WinHTTP, clipboard |
| [serde](https://github.com/serde-rs/serde) | MIT OR Apache-2.0 | Settings and library files |
| [serde_json](https://github.com/serde-rs/json) | MIT OR Apache-2.0 | JSON settings, library, and AI replies |
| [sha2](https://github.com/RustCrypto/hashes) | MIT OR Apache-2.0 | SHA-256 of original downloads |
| [winres](https://github.com/mxre/winres) | MIT | Icon and manifest at build time |
| [Rust](https://www.rust-lang.org/) / LLVM | MIT / Apache-2.0 / NCSA | Compiler |

## Companion tools (place next to the exe)

| Project | License | Notes |
| --- | --- | --- |
| [FFmpeg](https://ffmpeg.org/) | LGPL / GPL, depending on the build | `ffmpeg.exe` and `ffprobe.exe`. Converts audio to 16 kHz WAV for Whisper, reads durations, and cuts and encodes the trimmed Ogg Opus file (`libopus`). Gyan builds are GPL-3.0. Tested with `9.0.2-essentials_build` from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/). |
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) | MIT | `whisper-cli.exe` with `whisper.dll`, `ggml.dll`, `ggml-base.dll`, and `ggml-cpu-*.dll`. Transcribes on the CPU and writes SRT. Tested with release `b5454`, `whisper-bin-x64.zip`. |
| Whisper models in ggml format ([download](https://huggingface.co/ggerganov/whisper.cpp/tree/main)) | MIT (OpenAI Whisper weights) | Any `ggml-*.bin` model in the exe folder; choose one in Options. Tested with `ggml-tiny.en.bin`. |

FFmpeg is a trademark of Fabrice Bellard / the FFmpeg project.

## Windows

Tahoma, Common Controls 6, and WinHTTP are part of Windows. This program uses them by name and does not redistribute Windows or its fonts.

## Style

The visual style is an original imitation of late-1990s game menu colours and bevels. No artwork, fonts, sounds, or other assets from any game are included, and the project is not affiliated with any game publisher.
