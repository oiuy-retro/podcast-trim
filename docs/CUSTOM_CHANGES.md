# Custom changes

- Only the newest episodes of a newly added show are NEW (`[episodes] new-on-add`, default 1); older ones are OLD and drawn dimmed. Right-click an episode to mark it, or every episode older than it, as seen or unseen. Shows from older libraries get this once on upgrade, after `library.json` is copied to `library-before-0.1.2.json`.

- Every AI prompt carries the episode's audio hash (first 12 hex digits of the original's SHA-256), and Check cuts refuses a reply whose `hash:` line is missing or doesn't match.

- Bulk prompt: one AI prompt for up to `[prompt] bulk-size` (default 3) transcribed episodes, picked in a resizable list. The reply gives each episode's cuts under its hash, and Approve and cut cuts each matching episode.

- Optional menu sounds: `[secret]` `menu-sounds = on` (and `menu-sounds-dir = auto` or a folder) in configuration.conf plays the GameUI button sounds (`buttonrollover.wav`, `buttonclick.wav`, `buttonclickrelease.wav`) from your own Half-Life install (`valve\sound\ui`) or a `sounds\` folder next to the exe. No game files are included.
