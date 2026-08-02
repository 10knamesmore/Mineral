# tagging 测试 fixture

`tone.*`:1 秒 440Hz 正弦波,`-map_metadata -1` 剥掉 encoder tag,四个容器各一份
(mp3 = ID3v2 路径,flac = Vorbis 路径,m4a = MP4 ilst 路径,aac = 裸 ADTS + 前置
ID3v2 路径)。重新生成(需要 ffmpeg):

```bash
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -map_metadata -1 -codec:a libmp3lame -q:a 4 tone.mp3
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -map_metadata -1 -codec:a flac tone.flac
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -map_metadata -1 -codec:a aac -movflags +faststart tone.m4a
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -map_metadata -1 -codec:a aac -f adts tone.aac
```
