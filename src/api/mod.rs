use anyhow::{anyhow, Result};
use isahc::{
    config::Configurable, cookies::CookieJar, http::Uri, AsyncReadResponseExt, HttpClient, Request,
};
use lazy_static::lazy_static;
use std::{cell::RefCell, collections::HashMap, time::Duration};
use urlqstring::QueryParams;

use encrypt::*;
use model::*;

use crate::{api::parse::parse_song_info, app::Song};

mod encrypt;
mod model;
mod parse;

static BASE_URL: &str = "https://music.163.com";

const TIMEOUT: u64 = 100;

const LINUX_USER_AGNET: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/60.0.3112.90 Safari/537.36";

const USER_AGENT_LIST: [&str; 14] = [
    "Mozilla/5.0 (iPhone; CPU iPhone OS 9_1 like Mac OS X) AppleWebKit/601.1.46 (KHTML, like Gecko) Version/9.0 Mobile/13B143 Safari/601.1",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 9_1 like Mac OS X) AppleWebKit/601.1.46 (KHTML, like Gecko) Version/9.0 Mobile/13B143 Safari/601.1",
    "Mozilla/5.0 (Linux; Android 5.0; SM-G900P Build/LRX21T) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/59.0.3071.115 Mobile Safari/537.36",
    "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/59.0.3071.115 Mobile Safari/537.36",
    "Mozilla/5.0 (Linux; Android 5.1.1; Nexus 6 Build/LYZ28E) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/59.0.3071.115 Mobile Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 10_3_2 like Mac OS X) AppleWebKit/603.2.4 (KHTML, like Gecko) Mobile/14F89;GameHelper",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 10_0 like Mac OS X) AppleWebKit/602.1.38 (KHTML, like Gecko) Version/10.0 Mobile/14A300 Safari/602.1",
    "Mozilla/5.0 (iPad; CPU OS 10_0 like Mac OS X) AppleWebKit/602.1.38 (KHTML, like Gecko) Version/10.0 Mobile/14A300 Safari/602.1",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.12; rv:46.0) Gecko/20100101 Firefox/46.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_5) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/59.0.3071.115 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_5) AppleWebKit/603.2.4 (KHTML, like Gecko) Version/10.1.1 Safari/603.2.4",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:46.0) Gecko/20100101 Firefox/46.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/51.0.2704.103 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/42.0.2311.135 Safari/537.36 Edge/13.1058",
];

lazy_static! {
    static ref UA_ANY: UserAgentType = UserAgentType::Any;
    static ref UA_MOBILE: UserAgentType = UserAgentType::Mobile;
    static ref UA_PC: UserAgentType = UserAgentType::PC;
}

pub struct NcmApi {
    client: HttpClient,
    csrf: RefCell<String>,
}

enum CryptoApi {
    Weapi,
    LinuxApi,
    Eapi,
}

impl Default for NcmApi {
    fn default() -> Self {
        Self::new(0)
    }
}

impl NcmApi {
    pub fn new(max_cons: usize) -> Self {
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(TIMEOUT))
            .max_connections(max_cons)
            .cookies()
            .build()
            .expect("初始化网络请求失败");

        Self {
            client,
            csrf: RefCell::new(String::new()),
        }
    }

    pub fn from_cookie_jar(cookie_jar: CookieJar, max_cons: usize) -> Self {
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(TIMEOUT))
            .max_connections(max_cons)
            .cookies()
            .cookie_jar(cookie_jar)
            .build()
            .expect("初始网络化请求失败");

        Self {
            client,
            csrf: RefCell::new(String::new()),
        }
    }

    pub fn cookie_jar(&self) -> Option<&CookieJar> {
        self.client.cookie_jar()
    }

    /// 设置使用代理
    /// proxy: 代理地址，支持以下协议
    ///   - http: Proxy. Default when no scheme is specified.
    ///   - https: HTTPS Proxy. (Added in 7.52.0 for OpenSSL, GnuTLS and NSS)
    ///   - socks4: SOCKS4 Proxy.
    ///   - socks4a: SOCKS4a Proxy. Proxy resolves URL hostname.
    ///   - socks5: SOCKS5 Proxy.
    ///   - socks5h: SOCKS5 Proxy. Proxy resolves URL hostname.
    pub fn set_proxy(&mut self, proxy: &str) -> Result<()> {
        if let Some(cookie_jar) = self.client.cookie_jar() {
            let client = HttpClient::builder()
                .timeout(Duration::from_secs(TIMEOUT))
                .proxy(Some(proxy.parse()?))
                .cookies()
                .cookie_jar(cookie_jar.to_owned())
                .build()
                .expect("初始化网络请求失败!");
            self.client = client;
        } else {
            let client = HttpClient::builder()
                .timeout(Duration::from_secs(TIMEOUT))
                .proxy(Some(proxy.parse()?))
                .cookies()
                .build()
                .expect("初始化网络请求失败!");
            self.client = client;
        }
        Ok(())
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        params: HashMap<&str, &str>,
        cryptoapi: CryptoApi,
        ua_type: &UserAgentType,
        append_csrf: bool,
    ) -> Result<String> {
        let mut csrf = self.csrf.borrow().to_owned();

        if csrf.is_empty() {
            if let Some(cookie) = self.cookie_jar() {
                let uri: Uri = BASE_URL.parse().unwrap();
                if let Some(cookie) = cookie.get_by_name(&uri, "__csrf") {
                    let __csrf = cookie.value().to_string();
                    self.csrf.replace(__csrf.to_owned());
                    csrf = __csrf;
                }
            }
        }

        let mut url = if append_csrf {
            format!("{}{}?csrf_token={}", BASE_URL, path, csrf)
        } else {
            format!("{}{}", BASE_URL, path)
        };

        match method {
            Method::Get => self
                .client
                .get_async(&url)
                .await
                .map_err(|_| anyhow!("none"))?
                .text()
                .await
                .map_err(|_| anyhow!("none")),
            Method::Post => {
                let (user_agent, body) = match cryptoapi {
                    CryptoApi::Weapi => {
                        let ua = Self::choose_user_agent(ua_type).to_string();

                        let mut params = params;
                        params.insert("csrf_token", &csrf);
                        (ua, Crypto::weapi(&QueryParams::from_map(params).json()))
                    }
                    CryptoApi::LinuxApi => {
                        let ua = LINUX_USER_AGNET.to_string();

                        let data = format!(
                            r#"{{"method":"linuxapi","url":"{}","params":{}}}"#,
                            url.replace("weapi", "api"),
                            QueryParams::from_map(params).json()
                        );

                        (ua, Crypto::linuxapi(&data))
                    }
                    CryptoApi::Eapi => {
                        let ua = Self::choose_user_agent(ua_type).to_string();

                        let mut params = params;
                        params.insert("csrf_token", &csrf);
                        url = path.to_string();
                        (
                            ua,
                            Crypto::eapi(
                                "/api/song/enhance/player/url",
                                &QueryParams::from_map(params).json(),
                            ),
                        )
                    }
                };

                let request = Request::post(&url)
                    .header("Cookie", "os=pc; appver=2.7.1.198277")
                    .header("Accept", "*/*")
                    .header("Accept-Language", "en-US,en;q=0.5")
                    .header("Connection", "keep-alive")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Host", "music.163.com")
                    .header("Referer", "https://music.163.com")
                    .header("User-Agent", user_agent)
                    .body(body)
                    .unwrap();

                let mut response = self
                    .client
                    .send_async(request)
                    .await
                    .map_err(|_| anyhow!("none"))?;

                response.text().await.map_err(|_| anyhow!("none"))
            }
        }
    }

    fn choose_user_agent(ua_type: &UserAgentType) -> &str {
        let idx = match ua_type {
            UserAgentType::Any => rand::random::<u16>() % USER_AGENT_LIST.len() as u16,
            UserAgentType::Custom(ua) => return ua,
            UserAgentType::Mobile => rand::random::<u16>() % 7,
            UserAgentType::PC => rand::random::<u16>() % 5 + 8,
        };

        USER_AGENT_LIST[idx as usize]
    }

    /// 搜索
    /// keywords: 关键词
    /// types: 1: 单曲, 10: 专辑, 100: 歌手, 1000: 歌单, 1002: 用户, 1004: MV, 1006: 歌词, 1009: 电台, 1014: 视频
    /// offset: 起始点
    /// limit: 数量
    async fn search(
        &self,
        keywords: impl Into<String>,
        search_type: SearchType,
        offset: u16,
        limit: u16,
    ) -> Result<String> {
        let path = "/weapi/search/get";

        let keywords = keywords.into();
        let search_type: String = search_type.into();
        let offset = offset.to_string();
        let limit = limit.to_string();

        let mut params = HashMap::new();
        params.insert("s", &keywords[..]);
        params.insert("type", &search_type[..]);
        params.insert("offset", &offset[..]);
        params.insert("limit", &limit[..]);

        self.request(Method::Post, path, params, CryptoApi::Weapi, &UA_ANY, true)
            .await
    }

    pub async fn search_song(
        &self,
        keywords: impl Into<String>,
        offset: u16,
        limit: u16,
    ) -> Result<Vec<Song>> {
        let res = self
            .search(keywords, SearchType::Song, offset, limit)
            .await?;
        parse_song_info(res)
    }
}
