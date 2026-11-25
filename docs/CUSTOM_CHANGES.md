# Custom changes

- Only the newest episodes of a newly added show are NEW (`[episodes] new-on-add`, default 1); older ones are OLD and drawn dimmed. SENT episodes count as seen and are dimmed too, keeping their SENT tag. Right-click an episode to mark it, or every episode older than it, as seen or unseen. Shows from older libraries get this once on upgrade, after `library.json` is copied to `library-before-0.1.2.json`.

- Every AI prompt carries the episode's audio hash (first 12 hex digits of the original's SHA-256). Check cuts and Approve and cut apply a single-hash reply to the transcribed episode that hash names, even if another episode is selected; a reply is refused when the hash is missing or does not belong to a transcribed episode.

- Bulk prompt: one AI prompt for up to `[prompt] bulk-size` (default 3) transcribed episodes, picked in a resizable list. The reply gives each episode's cuts under its hash, and Approve and cut cuts each matching episode.

- Optional menu sounds: `[secret]` `menu-sounds = on` (and `menu-sounds-dir = auto` or a folder) in configuration.conf plays the GameUI button sounds (`buttonrollover.wav`, `buttonclick.wav`, `buttonclickrelease.wav`) from your own Half-Life install (`valve\sound\ui`) or a `sounds\` folder next to the exe. No game files are included.

- Download + transcribe queue: Shift/Ctrl-click several episodes and they are processed one at a time. Pending jobs are saved in `queue.json` next to `library.json`. Cancel stops the current job and the rest of the queue. Already-transcribed episodes and duplicates are skipped; a failure does not stop the jobs after it.

- Show and episode artwork from the source RSS feed (`itunes:image`, RSS 2.0 `<image>`, `media:thumbnail`, image `media:content`) is downloaded at add/refresh, badged with white-on-black `TRIMMED` using the system Impact font, cached under `shows/<id>/art/` by source hash and style version, and served from `/art/<show-id>/<hash>.jpg` in each trimmed feed (`<itunes:image>` and `<image>`). Episodes without their own art inherit the channel cover, and episodes matching the channel art URL reuse the channel file.

- Queue NEW episodes: the arrow next to Check for new (Check for new + queue NEW), right-click on a show (Queue NEW in this show, Check for new + queue NEW), the Queue N NEW link in the Episode header, and `[episodes] queue-after-refresh` (Options: Queue NEW after Check for new). OLD, SENT, transcribed, queued and running episodes are skipped.
