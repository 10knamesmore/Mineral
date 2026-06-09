//! 当前曲目元数据的本地缓存与写入:维护一份 `nowPlayingInfo` 字典,按需补丁
//! 进度 / 速率 / 封面后整体回写系统媒体中心。
//!
//! 本模块全程经 objc2 绑定调用 Objective-C 框架,`unsafe` 是 FFI 边界固有的;
//! 所有调用都在持有它的专属线程上发生,字典对象不跨线程传递。

#![allow(unsafe_code)]

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSImage;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
    MPMediaItemPropertyArtwork, MPMediaItemPropertyLyrics, MPMediaItemPropertyMediaType,
    MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPMediaType,
    MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyMediaType, MPNowPlayingInfoPropertyPlaybackRate,
};

use super::convert::{playback_rate, secs, to_now_playing_state};
use crate::state::{NowPlaying, PlaybackState};

/// 系统媒体中心的当前曲目状态持有者(单线程拥有,不可跨线程)。
pub(super) struct MacNowPlaying {
    /// 默认 Now Playing 中心单例。
    center: Retained<MPNowPlayingInfoCenter>,

    /// 当前 `nowPlayingInfo` 字典缓存:换歌重建、进度/速率/封面就地补丁后整体回写。
    info: Retained<NSMutableDictionary<NSString, AnyObject>>,

    /// 是否已有当前曲目:无歌时进度上报只设 Stopped、不动字典。
    has_song: bool,
}

impl MacNowPlaying {
    /// 取默认中心,起一份空字典。
    pub(super) fn new() -> Self {
        let center = unsafe { MPNowPlayingInfoCenter::defaultCenter() };
        let info = NSMutableDictionary::<NSString, AnyObject>::new();
        Self {
            center,
            info,
            has_song: false,
        }
    }

    /// 换歌 / 元数据变化:重建整张字典(清掉上一首的封面等),回写中心。
    ///
    /// 进度 / 速率不在此设,由随后的 [`Self::set_playback`] 补上;封面异步到达后由
    /// [`Self::set_artwork`] 补丁。
    pub(super) fn set_metadata(&mut self, np: &NowPlaying) {
        let info = NSMutableDictionary::<NSString, AnyObject>::new();
        if let Some(title) = &np.title {
            put_str(&info, unsafe { MPMediaItemPropertyTitle }, title);
        }
        if let Some(artist) = &np.artist {
            put_str(&info, unsafe { MPMediaItemPropertyArtist }, artist);
        }
        if let Some(album) = &np.album {
            put_str(&info, unsafe { MPMediaItemPropertyAlbumTitle }, album);
        }
        if let Some(duration) = np.duration {
            put_f64(
                &info,
                unsafe { MPMediaItemPropertyPlaybackDuration },
                secs(duration),
            );
        }
        let lrc = mineral_model::to_lrc_string(&np.original);
        if !lrc.is_empty() {
            put_str(&info, unsafe { MPMediaItemPropertyLyrics }, &lrc);
        }
        // 标成音乐类型:系统据此选 Control Center 的展示样式。
        let music = isize::try_from(MPMediaType::Music.0).unwrap_or(1);
        put_i64(&info, unsafe { MPMediaItemPropertyMediaType }, music);
        put_i64(
            &info,
            unsafe { MPNowPlayingInfoPropertyMediaType },
            // MPNowPlayingInfoMediaType:1 = Audio。
            1,
        );
        self.info = info;
        self.has_song = true;
        self.push();
    }

    /// 上报播放态与进度:补丁 elapsed + rate,并设播放态(macOS 靠它驱动控件)。
    pub(super) fn set_playback(&mut self, state: PlaybackState, position_secs: Option<f64>) {
        if !self.has_song {
            unsafe {
                self.center
                    .setPlaybackState(to_now_playing_state(PlaybackState::Stopped));
            }
            return;
        }
        if let Some(elapsed) = position_secs {
            put_f64(
                &self.info,
                unsafe { MPNowPlayingInfoPropertyElapsedPlaybackTime },
                elapsed,
            );
        }
        put_f64(
            &self.info,
            unsafe { MPNowPlayingInfoPropertyPlaybackRate },
            playback_rate(state),
        );
        self.push();
        unsafe { self.center.setPlaybackState(to_now_playing_state(state)) };
    }

    /// 非线性跳变后重设进度基准:只补 elapsed,速率不动。
    pub(super) fn seeked(&mut self, position_secs: f64) {
        if !self.has_song {
            return;
        }
        put_f64(
            &self.info,
            unsafe { MPNowPlayingInfoPropertyElapsedPlaybackTime },
            position_secs,
        );
        self.push();
    }

    /// 设置封面:把编码图片字节解成 `NSImage` → `MPMediaItemArtwork`,补丁进字典。
    pub(super) fn set_artwork(&mut self, image_bytes: &[u8]) {
        if !self.has_song {
            return;
        }
        let Some(artwork) = build_artwork(image_bytes) else {
            return;
        };
        let key = unsafe { MPMediaItemPropertyArtwork };
        let value: Retained<AnyObject> = unsafe { Retained::cast_unchecked(artwork) };
        unsafe {
            self.info
                .setObject_forKey(&value, ProtocolObject::from_ref(key));
        }
        self.push();
    }

    /// 把当前字典整体回写系统媒体中心。
    fn push(&self) {
        unsafe { self.center.setNowPlayingInfo(Some(&self.info)) };
    }
}

/// 往字典塞一个字符串值。
fn put_str(info: &NSMutableDictionary<NSString, AnyObject>, key: &NSString, value: &str) {
    let v = NSString::from_str(value);
    let v: Retained<AnyObject> = unsafe { Retained::cast_unchecked(v) };
    unsafe { info.setObject_forKey(&v, ProtocolObject::from_ref(key)) };
}

/// 往字典塞一个浮点值(包成 `NSNumber`)。
fn put_f64(info: &NSMutableDictionary<NSString, AnyObject>, key: &NSString, value: f64) {
    let v = NSNumber::numberWithDouble(value);
    let v: Retained<AnyObject> = unsafe { Retained::cast_unchecked(v) };
    unsafe { info.setObject_forKey(&v, ProtocolObject::from_ref(key)) };
}

/// 往字典塞一个整数值(包成 `NSNumber`)。
fn put_i64(info: &NSMutableDictionary<NSString, AnyObject>, key: &NSString, value: isize) {
    let v = NSNumber::numberWithInteger(value);
    let v: Retained<AnyObject> = unsafe { Retained::cast_unchecked(v) };
    unsafe { info.setObject_forKey(&v, ProtocolObject::from_ref(key)) };
}

/// 编码图片字节 → `MPMediaItemArtwork`;解码失败返回 `None`。
///
/// 用 `initWithBoundsSize:requestHandler:`:requestHandler 对任意请求尺寸都返回同一张
/// 已解码图(系统按需缩放)。图被 block 捕获持有,artwork 存活期间指针有效。
fn build_artwork(image_bytes: &[u8]) -> Option<Retained<MPMediaItemArtwork>> {
    let data = NSData::with_bytes(image_bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    let size = image.size();
    let handler = RcBlock::new(move |_requested: CGSize| -> core::ptr::NonNull<NSImage> {
        core::ptr::NonNull::from(&*image)
    });
    let artwork = unsafe {
        MPMediaItemArtwork::initWithBoundsSize_requestHandler(
            MPMediaItemArtwork::alloc(),
            size,
            &handler,
        )
    };
    Some(artwork)
}
