# Custom changes

- Optional menu sounds: `[secret]` `menu-sounds = on` (and `menu-sounds-dir = auto` or a folder) in configuration.conf plays the GameUI button sounds (`buttonrollover.wav`, `buttonclick.wav`, `buttonclickrelease.wav`) from your own Half-Life install (`valve\sound\ui`) or a `sounds\` folder next to the exe. No game files are included.

- Download + transcribe queue: Shift/Ctrl-click several episodes and they are processed one at a time. Pending jobs are saved in `queue.json` next to `library.json`. Cancel stops the current job and the rest of the queue. Already-transcribed episodes and duplicates are skipped; a failure does not stop the jobs after it.
