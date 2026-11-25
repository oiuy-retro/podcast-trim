# Custom changes

- Optional menu sounds: `[secret]` `menu-sounds = on` (and `menu-sounds-dir = auto` or a folder) in configuration.conf plays the GameUI button sounds (`buttonrollover.wav`, `buttonclick.wav`, `buttonclickrelease.wav`) from your own Half-Life install (`valve\sound\ui`) or a `sounds\` folder next to the exe. No game files are included.

- Show and episode artwork from the source RSS feed (`itunes:image`, RSS 2.0 `<image>`, `media:thumbnail`, image `media:content`) is downloaded at add/refresh, badged with white-on-black `TRIMMED` using the system Impact font, cached under `shows/<id>/art/` by source hash and style version, and served from `/art/<show-id>/<hash>.jpg` in each trimmed feed (`<itunes:image>` and `<image>`). Episodes without their own art inherit the channel cover, and episodes matching the channel art URL reuse the channel file.
