//! 媒体服务初始化配置。

use typed_builder::TypedBuilder;

/// [`crate::MediaService`] 初始化配置。
///
/// 字段私有 + builder 构造,遵循「不暴露可被字面量直接构造的配置 struct」。
///
/// 字段目前仅 Linux(MPRIS)后端消费(总线名 / identity);非 Linux 后端不需要它们,
/// 故对非 Linux 平台放开 `dead_code`。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, TypedBuilder)]
#[non_exhaustive]
pub struct MediaConfig {
    /// D-Bus 名后缀:MPRIS 总线名为 `org.mpris.MediaPlayer2.{dbus_name}`。
    /// 仅 Linux 后端使用。
    #[builder(setter(into))]
    pub(crate) dbus_name: String,

    /// 用户可见的播放器名(系统媒体控件里显示)。
    #[builder(setter(into))]
    pub(crate) display_name: String,
}
