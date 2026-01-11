use base64::{engine::general_purpose, Engine};
use once_cell::sync::Lazy;
use openssl::{
    hash::{hash, MessageDigest},
    rsa::{Padding, Rsa},
    symm::{encrypt, Cipher},
};
use urlqstring::QueryParams;

/// 初始化向量(IV)，用于AES加密
pub static IV: Lazy<Vec<u8>> = Lazy::new(|| b"0102030405060708".to_vec());
/// Linux API加密密钥
pub static LINUX_API_KEY: Lazy<Vec<u8>> = Lazy::new(|| b"rFgB&h#%2?^eDg:Q".to_vec());
/// 预设密钥，用于网页API的第一次AES加密
pub static PRESET_KEY: Lazy<Vec<u8>> = Lazy::new(|| b"0CoJUm6Qyw8W8jud".to_vec());
/// BASE62字符集，用于生成随机密钥
pub static BASE62: Lazy<Vec<u8>> =
    Lazy::new(|| b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_vec());
/// RSA公钥，用于加密客户端生成的随机密钥
pub static RSA_PUBLIC_KEY: Lazy<Vec<u8>> = Lazy::new(|| {
    b"-----BEGIN PUBLIC KEY-----\nMIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDgtQn2JZ34ZC28NWYpAUd98iZ37BUrX/aKzmFbt7clFSs6sXqHauqKWqdtLkF2KexO40H1YTX8z2lSgBBOAxLsvaklV8k4cBFK9snQXE9/DDaFt6Rr7iVZMldczhC0JNgTz+SHXT6CBHuX3e9SdB1Ua44oncaTWz7OBGLbCiK45wIDAQAB\n-----END PUBLIC KEY-----"
        .to_vec()
});
/// EAPI加密密钥，用于移动端API加密
pub static EAPIKEY: Lazy<Vec<u8>> = Lazy::new(|| b"e82ckenh8dichen8".to_vec());

/// 加密算法实现结构体
pub struct Crypto;

/// AES加密模式枚举
#[allow(non_camel_case_types)]
pub enum AesMode {
    /// CBC模式 - 密码分组链接模式
    cbc,
    /// ECB模式 - 电子密码本模式
    ecb,
}

impl Crypto {
    /// Linux API加密实现
    ///
    /// # Arguments
    /// * `text` - 待加密的文本
    ///
    /// # Returns
    /// * `String` - 加密后的查询字符串
    pub fn linuxapi(text: &str) -> String {
        // 使用LINUX_API_KEY进行ECB模式AES加密，结果转为大写十六进制
        let params =
            Crypto::aes_encrypt(text, &LINUX_API_KEY, AesMode::ecb, None, |t: &Vec<u8>| {
                hex::encode(t)
            })
            .to_uppercase();

        // 构造查询参数
        QueryParams::from(vec![("eparams", params.as_str())]).stringify()
    }

    /// 移动端API(EAPI)加密实现
    ///
    /// # Arguments
    /// * `url` - 请求的URL
    /// * `text` - 待加密的数据
    ///
    /// # Returns
    /// * `String` - 加密后的查询字符串
    pub fn eapi(url: &str, text: &str) -> String {
        // 构造消息字符串
        // 格式为: "nobody{url}use{text}md5forenctypt"
        let message = format!("nobody{}use{}md5forencrypt", url, text);

        // 计算消息的MD5哈希值并转换为十六进制字符串
        let digest = hex::encode(hash(MessageDigest::md5(), message.as_bytes()).unwrap());

        // 构造最终的数据字符串
        // 格式为: "{url}-36cd479b6b5-{text}-36cd479b6b5-{md5}"
        // 其中36cd479b6b5是固定的分隔符
        let data = format!("{}-36cd479b6b5-{}-36cd479b6b5-{}", url, text, digest);

        // 使用AES-128-ECB模式加密数据
        // EAPIKEY: 加密密钥
        // IV: 初始化向量
        // 加密结果使用大写十六进制编码
        let params =
            Crypto::aes_encrypt(&data, &EAPIKEY, AesMode::ecb, Some(&*IV), |t: &Vec<u8>| {
                hex::encode_upper(t)
            });

        // 将加密后的参数构造成URL查询字符串
        // 格式为: "params={encrypted_data}"
        QueryParams::from(vec![("params", params.as_str())]).stringify()
    }

    /// 网页端API(WEAPI)加密实现
    ///
    /// # Arguments
    /// * `text` - 待加密的文本
    ///
    /// # Returns
    /// * `String` - 加密后的查询字符串，包含params和enSecKey两个参数
    pub fn weapi(text: &str) -> String {
        // 生成16字节的随机密钥
        let mut secret_key = [0u8; 16];
        rand::fill(&mut secret_key[..]);

        // 将随机密钥转换为BASE62字符集的字符串
        // BASE62字符集包含a-z,A-Z,0-9共62个字符
        let key: Vec<u8> = secret_key
            .iter()
            .map(|i| BASE62[(i % 62) as usize])
            .collect();

        // 第一次AES加密
        // 使用预设密钥PRESET_KEY对原始文本进行AES-128-CBC加密
        // 并将结果进行Base64编码
        let params = Crypto::aes_encrypt(
            text,
            &PRESET_KEY,
            AesMode::cbc,
            Some(&*IV),
            |t: &Vec<u8>| general_purpose::STANDARD.encode(t),
        );

        // 第二次AES加密
        // 使用随机生成的key对第一次加密的结果进行AES-128-CBC加密
        // 并将结果再次进行Base64编码
        let params = Crypto::aes_encrypt(&params, &key, AesMode::cbc, Some(&*IV), |t: &Vec<u8>| {
            general_purpose::STANDARD.encode(t)
        });

        // RSA加密
        // 将随机生成的key反转后使用RSA公钥加密
        // 得到加密后的密钥enc_sec_key
        let enc_sec_key = Crypto::rsa_encrypt(
            std::str::from_utf8(&key.iter().rev().copied().collect::<Vec<u8>>()).unwrap(),
            &RSA_PUBLIC_KEY,
        );

        // 构造最终的查询参数
        // 包含两个参数：
        // 1. params: 两次AES加密后的数据
        // 2. enSecKey: RSA加密后的密钥
        QueryParams::from(vec![
            ("params", params.as_str()),
            ("encSecKey", enc_sec_key.as_str()),
        ])
        .stringify()
    }

    /// AES加密通用实现
    ///
    /// # Arguments
    /// * `data` - 待加密数据
    /// * `key` - 加密密钥
    /// * `mode` - 加密模式(CBC/ECB)
    /// * `iv` - 初始化向量(CBC模式需要)
    /// * `encode` - 编码函数，用于处理加密后的数据
    ///
    /// # Returns
    /// * `String` - 加密并编码后的字符串
    pub fn aes_encrypt(
        data: &str,
        key: &[u8],
        mode: AesMode,
        iv: Option<&[u8]>,
        encode: fn(&Vec<u8>) -> String,
    ) -> String {
        // 根据模式选择加密算法
        let cipher = match mode {
            AesMode::cbc => Cipher::aes_128_cbc(),
            AesMode::ecb => Cipher::aes_128_ecb(),
        };

        // 执行加密
        let cipher_text = encrypt(cipher, key, iv, data.as_bytes()).unwrap();

        // 对加密结果进行编码
        encode(&cipher_text)
    }

    /// RSA加密实现
    ///
    /// # Arguments
    /// * `data` - 待加密数据
    /// * `key` - RSA公钥
    ///
    /// # Returns
    /// * `String` - 加密后的十六进制字符串
    pub fn rsa_encrypt(data: &str, key: &[u8]) -> String {
        // 加载RSA公钥
        let rsa = Rsa::public_key_from_pem(key).unwrap();

        // 创建前缀填充
        let prefix = vec![0u8; 128 - data.len()];

        // 拼接数据
        let data = [&prefix[..], data.as_bytes()].concat();

        // 创建输出缓冲区
        let mut buf = vec![0; rsa.size() as usize];

        // 执行RSA加密
        rsa.public_encrypt(&data, &mut buf, Padding::NONE).unwrap();

        // 转换为十六进制字符串
        hex::encode(buf)
    }
}
