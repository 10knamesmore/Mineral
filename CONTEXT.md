# Mineral

这里定义 Mineral 在多 source catalog、library 与 playback 中共享的 ubiquitous language，使来源身份、取数和播放资源的责任边界保持一致。

## Language

**Source**:
音乐实体的来源身份，属于 ID namespace；它回答数据来自哪里，不负责执行取数。
_Avoid_: Channel, backend

**Channel**:
服务一个 source 的 catalog、library 与 user-data connector；它不负责打开音频资源。
_Avoid_: Source, Playback provider

**Playback provider**:
服务一个 source 的播放资源 provider；它把 song identity 解析为 source-neutral prepared playback，并隐藏该 source 的鉴权、取流、decrypt 与 preprocessing。
_Avoid_: Channel, Audio engine

**Prepared playback**:
已经解析、尚未打开且只能消费一次的 playback plan；它可以带有 optional Direct media，但不暴露 source-private DTO、credential、key 或 protocol representation。
_Avoid_: Play URL, Opened media

**Playback instance**:
从一次 current play 或 prefetch intent 创建到结束的唯一播放实例；即使 SongId 相同也属于不同 instance，prefetch 提升为 current 时保持同一 identity。
_Avoid_: Song, Queue item, Current version

**Raw media**:
Playback provider 内部尚未完成 source-specific preparation 的音频资源；它可能加密、混淆、分片或使用非 decoder-ready 封装，不是 consumer contract。
_Avoid_: Opened media, PCM, Direct media

**Media preparation**:
Playback provider 把 Raw media 转成 Opened media 的 source-specific processing；可包含 decrypt、deobfuscate、decompress、segment assembly 与必要的 demux/remux，但不包含 audio decode to PCM。
_Avoid_: Audio decoding, Playback

**Opened media**:
已经完成 source-specific 获取与 preprocessing、可直接交给统一 decoder 的 encoded audio resource；它显式声明 Forward-only 或 Random-access，不默认支持任意 seek。
_Avoid_: PCM, Play URL

**Direct media**:
带有 Direct locator 与 source-neutral media facts 的 optional capability；它服务 provenance、copy 与 rewrite，不是 playback、cache 或 export 的前置条件。
_Avoid_: Playback provider, Opened media

**Direct locator**:
Direct media 的直接访问位置，分为 remote locator 与 local path；它描述媒体在哪里，不描述音乐实体来自哪个 Source。
_Avoid_: Source, Raw media

**Local media hit**:
Mineral 在 cache 或 download library 中找到、准备作为 decoder input 打开的 encoded media candidate；它保留 song 原有 Source identity，不等于 Local source。
_Avoid_: Local source, Playback provider result

**Scan function**:
本地 library 的可替换形成规则；它决定文件系统内容如何成为 playlists，以及在文件 metadata 上应用哪些 projection patches。
_Avoid_: Fixed scanner

**Playlist directory**:
直接包含可识别 song 文件、并被 Scan function 选作一个 playlist 边界的 directory；descendant directories 是独立候选，不会隐式成为该 playlist 的成员。
_Avoid_: Recursive playlist, scan root

**Metadata override**:
scan function 在默认文件 metadata 形成后应用的 projection patch；它改变 Mineral 展示和检索的 metadata，不回写音频文件。
_Avoid_: Tag write, metadata replacement

**Collection index**:
Channel 形成的 canonical Album 或 Playlist snapshot 中，一条 Song membership 的 0-based absolute coordinate。它只描述该 snapshot 内的顺序，不是稳定 identity；source reorder 或 channel projection 改变后可以变化。
_Avoid_: Release number, Track number, Disc number, Queue index, View index, Cursor index

**Playlist entry**:
Playlist snapshot 中一条携带 Collection index 与 Song 的 membership。它不是 Song 本身，也不承诺跨 snapshot 稳定。
_Avoid_: Playlist song, Queue item

**Album track**:
Album snapshot 中一条携带 Collection index 与 Song 的 membership。它不包含 release numbering；channel 如需使用 track / disc label，只在 adapter 内处理，不把它们写入 shared model。
_Avoid_: Album song, Track number, Disc number
