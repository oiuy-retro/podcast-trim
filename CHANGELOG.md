# Changelog

## 0.1.2

- Auto Opus stays at 96 kbps for stereo and 64 kbps for mono (instead of halving a second time).
- Cut previews show the transcript words inside each cut.
- New shows list only their newest episode as NEW (set with **NEW on add** in Options or `[episodes] new-on-add` in `configuration.conf`). Older episodes are listed as OLD in dimmer text. Episodes found later by Check for new are NEW.
- SENT episodes count as seen and are drawn dimmed like OLD, keeping their SENT tag. Episodes sent before this version are marked seen once on upgrade.
- Right-click menu on the episode list: Mark as seen, Mark as unseen, Mark older as seen, Mark older as unseen.
- Libraries from 0.1.1 get the NEW rule once on first start; `library.json` is copied to `library-before-0.1.2.json` first. Older `library.json` files still load.
- The AI prompt includes the episode's audio hash and asks the AI to start its reply with `hash: <hash>`. Check cuts and Approve and cut apply a single-hash reply to the transcribed episode that hash names, even if another episode is selected. A reply is refused when the hash is missing or does not belong to a transcribed episode.
- **Bulk prompt...** copies one prompt for several transcribed episodes (at most **Bulk prompt size**, default 3, also `[prompt] bulk-size`). Check cuts matches each part of the reply to its episode by hash, and Approve and cut cuts them one after another.
- Queue NEW episodes: **Check for new + queue NEW** from the small arrow next to Check for new or from the show list's right-click menu, **Queue NEW in this show** on that menu, and a **Queue N NEW** link in the Episode list header while NEW episodes are waiting. **Queue NEW after Check for new** in Options (`[episodes] queue-after-refresh`, off by default) does it after every check. Only NEW episodes are queued.
- Show and episode artwork from the source RSS feed is downloaded at add/refresh, badged at the bottom with **TRIMMED** in the system Impact font, cached under the show's folder, and served by the local feed server.
- Corrupt or unreadable `settings.json`, `library.json`, or `queue.json` files are backed up / repaired on startup instead of crashing, and startup diagnostics write `podcast-trim-trace.log` and `crash.log` next to the exe if anything goes wrong.

## 0.1.1

- Check for new reads every show's feed at once.
- The window can be resized and maximized and remembers its place.
- A maximized window fills the monitor's work area.

## 0.1.0

- First release.
