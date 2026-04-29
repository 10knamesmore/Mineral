//! 网易云加密用到的全部常量。

/// AES-CBC 第一层 key(WEAPI)。
pub const PRESET_KEY: &[u8; 16] = b"0CoJUm6Qyw8W8jud";

/// AES-CBC 用的 IV(WEAPI 两层都用这个;EAPI 不用 IV)。
pub const IV: &[u8; 16] = b"0102030405060708";

/// LINUXAPI AES-ECB key(含 `&`/`#`/`?`/`^`/`:` 等 URL 元字符,不要 URL 编码)。
pub const LINUX_API_KEY: &[u8; 16] = b"rFgB&h#%2?^eDg:Q";

/// EAPI AES-ECB key。
pub const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";

/// EAPI 拼接消息时左右两边的固定分隔符。
pub const EAPI_SEPARATOR: &str = "-36cd479b6b5-";

/// EAPI MD5 包裹三段。
pub const EAPI_MD5_PREFIX: &str = "nobody";
pub const EAPI_MD5_INFIX: &str = "use";
pub const EAPI_MD5_SUFFIX: &str = "md5forencrypt";

/// 随机字符集(BASE62),用于生成 16 字节随机 key 和 deviceId 字符串。
pub const STD_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// RSA 公钥 PEM(PKIX 格式,1024 bit)。
pub const RSA_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDgtQn2JZ34ZC28NWYpAUd98iZ3
7BUrX/aKzmFbt7clFSs6sXqHauqKWqdtLkF2KexO40H1YTX8z2lSgBBOAxLsvaklV
8k4cBFK9snQXE9/DDaFt6Rr7iVZMldczhC0JNgTz+SHXT6CBHuX3e9SdB1Ua44on
caTWz7OBGLbCiK45wIDAQAB
-----END PUBLIC KEY-----";
