# Podcast Trim

Windows app that cuts ads out of podcast episodes using a local Whisper transcript and an ad list from an AI you choose, then serves the trimmed episodes to your podcast app as one RSS feed per show.

## What it does

1. You add a show by pasting its RSS link. Podcast Trim reads the feed and lists its episodes.
2. **Download + transcribe** downloads an episode, records the SHA-256 hash of the file, and transcribes it on the CPU with [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Every transcript line has start and end times. A progress bar at the bottom of the window follows the download, the transcription, and the cutting, and Cancel stops the running step and its ffmpeg or whisper.cpp process.
3. **Copy AI prompt** copies a ready-made prompt plus the transcript to the clipboard. Paste it into any AI chat. No AI service is built in and nothing is sent anywhere by the app.
4. Paste the AI's reply into the reply box and press **Check cuts**. The reply must be a list of ad time ranges (format below). Podcast Trim never runs commands from the reply. It checks that every range is inside the episode, ends after it starts, and does not overlap another range, then shows the cut list and the exact ffmpeg command it will run.
5. **Approve and cut** runs ffmpeg itself (as an argument list, without a shell) and writes an Ogg Opus file (`.opus`) without the ads. The original download is deleted.
6. The trimmed episode appears in that show's feed at `http://<this-pc>:8737/feed/<show-id>.xml`. The link uses this PC's detected network address unless **PC address for phone** is set in Options. Subscribe to that link in AntennaPod or another podcast app.
7. Once a client has downloaded every byte of a trimmed file, the file is deleted and the episode is marked SENT. The app keeps the hash, the transcript, the cut list, and the ffmpeg arguments.
8. **Regenerate** on a SENT episode downloads the original again, checks that its hash matches, and rebuilds the trimmed file from the saved cuts without transcribing again or asking the AI again. If the publisher has changed the file (for example, new ads were inserted), the hash will not match, the old cuts are discarded, and the episode goes back to the transcribe step.

**Export Opus...** asks for a folder (it starts in the last one used) and copies the trimmed episode there, for use without the feed.

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

## Audio format

Trimmed episodes are Ogg Opus (`.opus`, encoded with libopus, variable bitrate). With **Opus bitrate** and **Channels** set to Auto (the defaults), both are picked from the original with ffprobe.

**Channels.** A mono source is encoded mono. For a stereo source, three 10-second samples (at 15%, 50% and 85% of the episode) are checked: if the difference between the channels (L-R) is at least 30 dB quieter than their sum (L+R), the two channels are the same and the episode is encoded mono. Real stereo is usually only 6 to 20 dB quieter; mono saved as stereo is 40 dB or more. Channels can also be set to Mono or Stereo.

**Bitrate.** The stereo bitrate comes from this table. Low-bitrate sources get lower tiers than Opus Convert uses, because Opus is about as good at 48k as MP3 is at 64k, so a trimmed file is never needlessly bigger than its source. From 129k up the tiers are the same as Opus Convert's.

| Source | Stereo Opus bitrate |
| --- | --- |
| MP3/WMA | ≤40 → 32k, 41–64 → 48k, 65–96 → 64k, 97–128 → 96k, 129–192 → 128k, 193–256 → 160k, 257+ → 192k |
| AAC/Vorbis | ≤40 → 32k, 41–64 → 48k, 65–96 → 64k, 97–128 → 96k, 129–192 → 128k, 193+ → 160k |
| AC-3 | ≤192 → 128k, 193–384 → 160k, 385+ → 192k |
| Lossless, DTS | 192k |
| Opus | its own bitrate, rounded up to 24, 32, 48, 64, 96, 128, 160 or 192k (cutting always re-encodes, so it can't be copied) |

Mono uses about 60% of the stereo bitrate, rounded up, and never less than 24k:

| Stereo | 32k | 48k | 64k | 96k | 128k | 160k | 192k |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Mono | 24k | 32k | 40k | 64k | 80k | 96k | 128k |

Details shows the result, for example `Source mp3 55 kbps stereo (identical channels) → Opus 32 kbps mono (Auto)`. A fixed bitrate (6 to 256 kbit/s) can be entered instead of Auto. Settings from older versions with the **Mono** checkbox ticked become Channels: Mono. Feeds list them with the MIME type `audio/ogg`, the registered type for Ogg Opus files (RFC 7845). AntennaPod plays them through its built-in player (ExoPlayer) on Android 5 and later.

## Run

Everything Podcast Trim needs must be in the same folder as `Podcast Trim.exe`:

| File | From |
| --- | --- |
| `ffmpeg.exe`, `ffprobe.exe` | FFmpeg, the `bin` folder of a [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) build |
| `whisper-cli.exe`, `whisper.dll`, `ggml.dll`, `ggml-base.dll`, `ggml-cpu-*.dll` | the `Release` folder of `whisper-bin-x64.zip` from the [whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases) (copying the whole folder is fine) |
| one or more `ggml-*.bin` models | [whisper.cpp models](https://huggingface.co/ggerganov/whisper.cpp/tree/main), for example `ggml-base.en.bin` |

1. Put `Podcast Trim.exe` and the files above in one folder.
2. Start `Podcast Trim.exe`. If anything is missing, the status line lists what is missing and where it goes, and transcribing and cutting stay unavailable until it is there.
3. If there is more than one model in the folder, press **Options**, open the **Whisper model** list and pick one, and press **Save**. Larger models are more accurate and slower on the CPU. `base.en` or `small.en` is a reasonable start for English shows.

Settings, the show list, and episode records are stored in `%AppData%\Podcast Trim`. Set the `PODCAST_TRIM_DATA` environment variable to use another folder.

Tested with FFmpeg `9.0.2-essentials_build` (Gyan) and whisper.cpp `b5454` (`whisper-bin-x64.zip`) with the `ggml-tiny.en.bin` model, under Wine.

## Build

The crate builds for `x86_64-pc-windows-gnu` by default (`.cargo/config.toml`). From Linux this needs the MinGW-w64 toolchain (`x86_64-w64-mingw32-gcc` and `x86_64-w64-mingw32-windres`).

```
cargo build --release
cp target/x86_64-pc-windows-gnu/release/podcast-trim.exe "dist/Podcast Trim.exe"
```

Release builds link the C runtime statically:

```
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release
```

Tests run the Windows build under Wine (the runner set in `.cargo/config.toml`), or natively on Linux:

```
cargo test
cargo test --target x86_64-unknown-linux-gnu --lib
cargo clippy --all-targets
```

`tests/e2e.rs` runs the whole download, transcribe, cut, deliver, and regenerate cycle with the real tools when `PT_TOOLS_DIR` (a folder with the same files as the app folder) and `PT_SAMPLE` (a short audio file, such as whisper.cpp's `samples/jfk.mp3`) are set. Otherwise it is skipped.

## Look

The window copies the general look of late-1990s game dialogs: olive panels, light text with khaki for the active tab and checked options, a mustard selection bar, 1-pixel light and dark bevels, and the app's own caption bar, tabs, buttons, and scroll bars. It is drawn with plain Win32 and GDI. Text uses Tahoma, which ships with Windows, at 13 pixels without smoothing. The progress bar copies the segmented bar of the game's loading dialog. The window can be resized from the grip in its bottom-right corner, down to 900 by 600 pixels. No game assets are included.

## License

[MIT](LICENSE) for this program. Companion tools keep their own licenses; see [THIRD_PARTY.md](THIRD_PARTY.md).
