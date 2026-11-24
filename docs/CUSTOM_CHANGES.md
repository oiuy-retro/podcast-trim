# Custom changes

- Optional menu sounds: `[secret]` `menu-sounds = on` (and `menu-sounds-dir = auto` or a folder) in configuration.conf plays the GameUI button sounds (`buttonrollover.wav`, `buttonclick.wav`, `buttonclickrelease.wav`) from your own Half-Life install (`valve\sound\ui`) or a `sounds\` folder next to the exe. No game files are included.

- Show and episode artwork from the source RSS feed (`itunes:image`, RSS 2.0 `<image>`, `media:thumbnail`, image `media:content`) is stored as `image_url` in `library.json` and passed on in each trimmed feed as `<itunes:image>` (and `<image>` for the show). Check for new fills it in for shows and episodes added earlier.
