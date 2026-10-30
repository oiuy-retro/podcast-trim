# Podcast Trim

Windows app that cuts ads out of podcast episodes using a local Whisper transcript and an ad list from an AI you choose, then serves the trimmed episodes to your podcast app as one RSS feed per show.

## What it does

1. You add a show by pasting its RSS link. Podcast Trim reads the feed and lists its episodes.
2. **Download + transcribe** downloads an episode, records the SHA-256 hash of the file, and transcribes it on the CPU with [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Every transcript line has start and end times.
3. **Copy AI prompt** copies a ready-made prompt plus the transcript to the clipboard. Paste it into any AI chat. No AI service is built in and nothing is sent anywhere by the app.
4. Paste the AI's reply into the reply box and press **Check cuts**. The reply must be a list of ad time ranges (format below). Podcast Trim never runs commands from the reply. It checks that every range is inside the episode, ends after it starts, and does not overlap another range, then shows the cut list and the exact ffmpeg command it will run.
5. **Approve and cut** runs ffmpeg itself (as an argument list, without a shell) and writes an MP3 without the ads. The original download is deleted.
6. The trimmed episode appears in that show's feed at `http://<this-pc>:8737/feed/<show-id>.xml`. Subscribe to that link in AntennaPod or another podcast app.
7. Once a client has downloaded every byte of a trimmed file, the file is deleted and the episode is marked SENT. The app keeps the hash, the transcript, the cut list, and the ffmpeg arguments.
8. **Regenerate** on a SENT episode downloads the original again, checks that its hash matches, and rebuilds the trimmed file from the saved cuts without transcribing again or asking the AI again. If the publisher has changed the file (for example, new ads were inserted), the hash will not match, the old cuts are discarded, and the episode goes back to the transcribe step.

**Export MP3** copies a trimmed episode to the export folder set in Options, for use without the feed.

## AI reply format

One ad per line, start and end separated by a hyphen:

```
00:00:00.000-00:00:42.500
00:23:10.120-00:25:01.900
```

`MM:SS.mmm` and plain seconds (`12.5-40`) are also accepted, as is a line `NONE` for no ads. Blank lines, lines starting with `#`, and Markdown code fences are ignored. JSON is also accepted:

```
{"ads":[{"start":"00:00:00.000","end":"00:00:42.500"}]}
```

Anything else is rejected with the line number of the problem.

## Feed server and access

- The server listens on `0.0.0.0:8737` by default, so phones on the same network can reach it. Set the listen address to `127.0.0.1` in Options to allow only this PC. Changes take effect after restarting the app.
- With **Require feed token** on (the default), each show has its own random token, and the feed and episode links only work with `?token=...` at the end. The link shown under **Feed link for your podcast app** already includes it. Treat that link like a password.
- Set **PC address for phone** in Options (for example `192.168.1.20`) so the displayed feed link is ready to paste. The links inside a feed use whatever address your podcast app used to reach it.
- The server speaks plain HTTP. To reach it from outside your home network, use a private VPN such as Tailscale instead of opening a port on your router.
- Windows Firewall will ask whether to allow Podcast Trim on the network the first time it starts. Allow it on private networks.

## Run

1. Copy `Podcast Trim.exe` to a folder.
2. Get FFmpeg from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) and put `ffmpeg.exe` and `ffprobe.exe` next to it, or enter their full paths in Options.
3. Get the whisper.cpp Windows build `whisper-bin-x64.zip` from the [whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases) and unzip it. In Options, set **whisper-cli.exe** to the full path of `Release\whisper-cli.exe`.
4. Download a model such as `ggml-base.en.bin` from [the whisper.cpp models page](https://huggingface.co/ggerganov/whisper.cpp/tree/main) and set **Whisper model** to its full path. Larger models are more accurate and slower on the CPU. `base.en` or `small.en` is a reasonable start for English shows.
5. Start `Podcast Trim.exe`, press **Options**, check the paths, and press **Save**.

Settings, the show list, and episode records are stored in `%AppData%\Podcast Trim`. Set the `PODCAST_TRIM_DATA` environment variable to use another folder.

Tested with FFmpeg `9.0.2-essentials_build` (Gyan) and whisper.cpp `b5454` (`whisper-bin-x64.zip`) with the `ggml-tiny.en.bin` model, under Wine.

## Build

The crate builds for `x86_64-pc-windows-gnu` by default (`.cargo/config.toml`). From Linux this needs the MinGW-w64 toolchain (`x86_64-w64-mingw32-gcc` and `x86_64-w64-mingw32-windres`).

```
cargo build --release
cp target/x86_64-pc-windows-gnu/release/podcast-trim.exe "dist/Podcast Trim.exe"
```

Tests run the Windows build under Wine (the runner set in `.cargo/config.toml`), or natively on Linux:

```
cargo test
cargo test --target x86_64-unknown-linux-gnu --lib
cargo clippy --all-targets
```

`tests/e2e.rs` runs the whole download, transcribe, cut, deliver, and regenerate cycle with the real tools when `PT_FFMPEG`, `PT_FFPROBE`, `PT_WHISPER`, `PT_MODEL`, and `PT_SAMPLE` (a short audio file, such as whisper.cpp's `samples/jfk.mp3`) are set. Otherwise it is skipped.

## Look

The window copies the general look of late-1990s game menus: olive panels, khaki headings, 1-pixel light and dark bevels, and flat beveled buttons. It is drawn with plain Win32 and GDI. Text uses Tahoma, a font that ships with Windows, without anti-aliasing. No game assets are included. Scroll bars are the standard Windows ones.

## License

[MIT](LICENSE) for this program. Companion tools keep their own licenses; see [THIRD_PARTY.md](THIRD_PARTY.md).
