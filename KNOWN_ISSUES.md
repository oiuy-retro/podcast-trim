# Known issues

Open copies live on the [Issues](https://github.com/oiuy-retro/podcast-trim/issues) tab.

## Playback at 2× and faster

[#3](https://github.com/oiuy-retro/podcast-trim/issues/3). Trimmed episodes are Opus. The bitrate ladder tops out at 192 kbps, and spoken shows usually land much lower (sources at or below 128 kbps use the low tiers, and matching stereo channels are encoded mono, down to 24 kbps). That quality is not good enough when the file is played at 2× or faster.
