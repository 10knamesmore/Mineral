/// channel 登录的凭证形式。具体 channel 通常只支持其中几种。
#[derive(Clone, Debug)]
pub enum Credential {
    /// 邮箱 + 明文密码,channel 内部决定是否做 md5 等预处理。
    EmailPassword {
        /// 注册邮箱。
        email: String,
        /// 明文密码。
        password: String,
    },
    /// 手机号 + 明文密码。
    PhonePassword {
        /// 国家区号(不含 `+`,例如 `86`)。
        country_code: String,
        /// 手机号(不含区号)。
        phone: String,
        /// 明文密码。
        password: String,
    },
    /// 二维码扫描成功后取得的 cookie 字符串。
    Cookie(String),
}
