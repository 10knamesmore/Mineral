/// channel 登录的凭证形式。具体 channel 通常只支持其中几种。
#[derive(Clone, Debug)]
pub enum Credential {
    /// 邮箱 + 明文密码,channel 内部决定是否做 md5 等预处理。
    EmailPassword { email: String, password: String },
    /// 手机号 + 明文密码。
    PhonePassword {
        country_code: String,
        phone: String,
        password: String,
    },
    /// 二维码扫描成功后取得的 cookie 字符串。
    Cookie(String),
}
