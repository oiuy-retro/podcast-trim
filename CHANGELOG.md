# Changelog

## Unreleased

- New shows list only their newest episode as NEW (set with **NEW on add** in Options or `[episodes] new-on-add` in `configuration.conf`). Older episodes are listed as OLD in dimmer text. Episodes found later by Check for new are NEW.
- SENT episodes count as seen and are drawn dimmed like OLD, keeping their SENT tag. Episodes sent before this version are marked seen once on upgrade.
- Right-click menu on the episode list: Mark as seen, Mark as unseen, Mark older as seen, Mark older as unseen.
- Libraries from 0.1.1 get the NEW rule once on first start; `library.json` is copied to `library-before-0.1.2.json` first. Older `library.json` files still load.
- The AI prompt includes the episode's audio hash and asks the AI to start its reply with `hash: <hash>`. Check cuts refuses replies without it or with another episode's hash.
- **Bulk prompt...** copies one prompt for several transcribed episodes (at most **Bulk prompt size**, default 3, also `[prompt] bulk-size`). Check cuts matches each part of the reply to its episode by hash, and Approve and cut cuts them one after another.

## 0.1.1

- Check for new reads every show's feed at once.
- The window can be resized and maximized and remembers its place.
- A maximized window fills the monitor's work area.

## 0.1.0

- First release.
