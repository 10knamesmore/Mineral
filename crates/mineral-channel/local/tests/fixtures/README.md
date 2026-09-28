# Local Lyrics Audio Fixtures

These generated silent recordings contain no lyric tags. `../lyrics.rs` copies
files into temporary libraries and writes each test's tags with Lofty. Tests do
not invoke ffmpeg and never modify the committed recordings.

| File | Audio Stream | Duration Requested | Bytes |
| --- | --- | --- | --- |
| `silence-mpeg1.mp3` | MPEG-1 Layer III, mono, 44,100 Hz, 32 kbit/s | 0.20 s | 940 |
| `silence.flac` | FLAC, mono, 44,100 Hz | 0.12 s | 8,353 |
| `silence.ogg` | Vorbis, stereo, 44,100 Hz | 0.12 s | 3,649 |
| `silence.m4a` | AAC-LC in MP4, mono, 44,100 Hz | 0.12 s | 877 |

Compressed streams can include encoder delay or whole-frame padding. The MPEG
fixture omits Xing and ID3 headers so its stream properties do not depend on
encoder metadata. Native Vorbis comments, MP4 ilst, and APEv2 on MP3 exercise
`ItemKey::Lyrics`; no Monkey's Audio encoder is required.

USLT and all outer ID3 structures are written by Lofty. FLAC secondary-tag cases
use `dump_to` and prepend the serialized tag because Lofty treats FLAC ID3v2 as
read-only. WAV cases use `mineral_test::write_wav` at runtime instead of
committed PCM data.

## Regeneration

Generated with Homebrew ffmpeg 9.0.2. Run these commands in this directory.
`-map_metadata -1` prevents input metadata copying; muxers can still write their
own encoder or vendor fields. The Ogg command uses FFmpeg's native Vorbis
encoder, so it does not require a build with libvorbis.

```bash
ffmpeg -y -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=mono' -t 0.20 -map_metadata -1 -codec:a libmp3lame -b:a 32k -write_xing 0 -id3v2_version 0 -write_id3v1 0 silence-mpeg1.mp3
ffmpeg -y -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=mono' -t 0.12 -map_metadata -1 -codec:a flac silence.flac
ffmpeg -y -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=stereo' -t 0.12 -map_metadata -1 -codec:a vorbis -strict -2 silence.ogg
ffmpeg -y -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=mono' -t 0.12 -map_metadata -1 -codec:a aac -b:a 32k -movflags +faststart silence.m4a
```

All four committed files were decoded successfully with
`ffmpeg -hide_banner -loglevel error -i FILE -f null -` after generation. File
sizes and container signatures were checked with `wc -c` and `file`.
