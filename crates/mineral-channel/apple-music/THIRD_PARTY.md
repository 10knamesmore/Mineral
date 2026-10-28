# Third-party notices

## Gamdl device identity

`assets/gamdl-android-l3.wvd` is the Base64-decoded `WVD` constant from
[`gamdl/interface/wvd.py`](https://github.com/glomatico/gamdl/blob/bc3bcd25ed61dcdb78c28ffb0ef32053b618c7e3/gamdl/interface/wvd.py)
at commit `bc3bcd25ed61dcdb78c28ffb0ef32053b618c7e3`. The decoded bytes are
unchanged. The upstream file states:

> Dumped from Android Studio Virtual Device running Android 9

The resource contains a shared Android L3 device private key and client
identity in WVD v2 format. It is used when `sources.am.device_file` is unset.
Revocation can prevent playback with this identity.

Gamdl declares the MIT license reproduced below. Its repository does not
document separate authorization to distribute the extracted device credentials.
No Gamdl Python implementation or pywidevine dependency is included.

### Enhanced HLS prefetch key

The CBCS reader uses Gamdl's published Apple prefetch key and selects the
prefetch or licensed key from each fragment's sample description. The reference
is [`gamdl/downloader/ammuxer/src/media.rs`](https://github.com/glomatico/gamdl/blob/bc3bcd25ed61dcdb78c28ffb0ef32053b618c7e3/gamdl/downloader/ammuxer/src/media.rs).
AES operations use RustCrypto.

### Gamdl — MIT

MIT License

Copyright (c) 2024 Glomatico

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## CENC format reference

The CENC implementation was written in Rust using the field layout and behavior
of Eyevinn mp4ff as a reference. No third-party source file is vendored.

### Eyevinn mp4ff — MIT

Copyright (c) 2019-2022 Edgeware AB, 2023- Eyevinn Technology AB

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
