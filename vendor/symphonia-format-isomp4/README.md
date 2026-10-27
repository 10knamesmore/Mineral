# Symphonia ISO/MP4 Demuxer

[<img alt="Docs.rs" src="https://img.shields.io/badge/docs.rs-symphonia_format_isomp4-brightgreen?style=for-the-badge" height="22"/>](https://docs.rs/symphonia-format-isomp4)

ISO/MP4 demuxer for Project Symphonia.

> [!NOTE]
> This crate is part of Project Symphonia. Please use the [`symphonia`](https://crates.io/crates/symphonia) crate instead of this one directly.

## License

Symphonia is provided under the MPL v2.0 license. Please refer to the LICENSE file for more details.

## Contributing

Symphonia is a free and open-source project that welcomes contributions! To get started, please read our [Contribution Guidelines](https://github.com/pdeljanov/Symphonia/blob/main/CONTRIBUTING.md).

## Local changes

Based on 0.6.1, with the MP4 fix from [upstream #340](https://github.com/pdeljanov/Symphonia/pull/340):
`src/demuxer.rs` uses `byte_len()` instead of `seek(End(0))` during initialization,
avoiding a whole-file download wait. The position save and restore are removed;
unknown lengths return `Unseekable`.
