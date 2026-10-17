<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# HLS test fixture

A six-second synthetic stream (ffmpeg `testsrc2` and a 440 Hz tone) laid out like
Cloudflare Stream's Farcaster videos: an fMP4 master playlist with a separate
audio rendition. It is VP9/Opus rather than H.264/AAC because Playwright's
Chromium has no proprietary codecs; the e2e test substitutes it for a real
video's playlist to prove the Rust Media Source Extensions player plays.

Regenerate with:

```sh
ffmpeg -f lavfi -i "testsrc2=size=320x180:rate=24:duration=6" -f lavfi -i "sine=frequency=440:duration=6" \
  -map 0:v -map 1:a -c:v libvpx-vp9 -b:v 120k -deadline realtime -cpu-used 8 -row-mt 1 -g 48 -keyint_min 48 \
  -c:a libopus -b:a 32k -f hls -hls_time 2 -hls_playlist_type vod -hls_segment_type fmp4 \
  -hls_fmp4_init_filename "init_%v.mp4" -hls_segment_filename "seg_%v_%d.m4s" -master_pl_name master.m3u8 \
  -var_stream_map "v:0,agroup:aud a:0,agroup:aud,default:yes" "stream_%v.m3u8"
```

then add `CODECS="vp09.00.10.08,opus"` to the `EXT-X-STREAM-INF` line of `master.m3u8`.
