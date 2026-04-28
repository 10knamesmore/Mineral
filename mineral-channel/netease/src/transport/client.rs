use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use isahc::{
    config::Configurable, cookies::CookieJar, http::Uri, AsyncReadResponseExt, HttpClient, Request,
};

use crate::config::NeteaseConfig;
use crate::crypto::{eapi, linuxapi, weapi};
use crate::transport::body::{decode_response, parse_code};
use crate::transport::headers::{pick_user_agent, UaKind, UA_LINUX};
use crate::transport::url::{rewrite, Crypto};

const BASE_URL: &str = "https://music.163.com";
const TIMEOUT_SECS: u64 = 100;

/// 一次请求的输入。
pub struct RequestSpec<'a> {
    pub path: &'a str,
    pub crypto: Crypto,
    pub params: serde_json::Map<String, serde_json::Value>,
    pub ua: UaKind,
}

/// HTTP 传输层:封装 isahc 客户端 + 全局 cookie jar + 加密 dispatcher。
pub struct Transport {
    client: HttpClient,
    csrf: Mutex<String>,
}

impl Transport {
    pub fn new(config: &NeteaseConfig) -> Result<Self> {
        let mut builder = HttpClient::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .max_connections(config.max_connections)
            .cookies();
        if let Some(p) = config.proxy.as_deref() {
            builder = builder.proxy(Some(p.parse().context("invalid proxy url")?));
        }
        let client = builder.build().context("build isahc client failed")?;
        Ok(Self {
            client,
            csrf: Mutex::new(String::new()),
        })
    }

    pub fn from_cookie_jar(config: &NeteaseConfig, jar: CookieJar) -> Result<Self> {
        let mut builder = HttpClient::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .max_connections(config.max_connections)
            .cookies()
            .cookie_jar(jar);
        if let Some(p) = config.proxy.as_deref() {
            builder = builder.proxy(Some(p.parse().context("invalid proxy url")?));
        }
        let client = builder.build().context("build isahc client failed")?;
        Ok(Self {
            client,
            csrf: Mutex::new(String::new()),
        })
    }

    pub fn cookie_jar(&self) -> Option<&CookieJar> {
        self.client.cookie_jar()
    }

    /// 直接走 dispatcher,跳过 `code != 200` 的检查;返回 (code, full_json)。
    /// 用于联调 example —— 即使端点返回非 200 也想看到完整 body 排查。
    pub async fn ping(&self, spec: RequestSpec<'_>) -> Result<(i64, serde_json::Value)> {
        let v = self.request_lax(spec).await?;
        let code = parse_code(&v);
        Ok((code, v))
    }

    /// 拿 csrf,优先用缓存的;若空则从 cookie jar 里读 `__csrf` 并缓存。
    fn csrf_token(&self) -> String {
        {
            let cached = self.csrf.lock().expect("csrf mutex poisoned");
            if !cached.is_empty() {
                return cached.clone();
            }
        }
        if let Some(jar) = self.cookie_jar() {
            let uri: Uri = BASE_URL.parse().unwrap();
            if let Some(cookie) = jar.get_by_name(&uri, "__csrf") {
                let val = cookie.value().to_string();
                *self.csrf.lock().expect("csrf mutex poisoned") = val.clone();
                return val;
            }
        }
        String::new()
    }

    /// 发请求并返回解析后的 JSON Value;`code != 200` 时返回 `Err`。
    pub async fn request(&self, spec: RequestSpec<'_>) -> Result<serde_json::Value> {
        let value = self.request_lax(spec).await?;
        let code = parse_code(&value);
        if code != 200 {
            return Err(anyhow!(
                "api code {code}: {}",
                value
                    .get("message")
                    .or_else(|| value.get("msg"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no message)")
            ));
        }
        Ok(value)
    }

    /// 发请求并返回解析后的 JSON,**不**因为 `code != 200` 报错。
    /// 用于 `CheckQR` 等用 `code` 表达业务状态的端点。
    pub async fn request_lax(&self, spec: RequestSpec<'_>) -> Result<serde_json::Value> {
        let csrf = self.csrf_token();

        // 注入 csrf_token 到 weapi/eapi 的 params(linuxapi 不注入)
        let mut params = spec.params;
        if matches!(spec.crypto, Crypto::Weapi | Crypto::Eapi) {
            params.insert("csrf_token".into(), serde_json::Value::String(csrf.clone()));
        }

        let url = rewrite(spec.path, spec.crypto);
        let json_text = serde_json::to_string(&params)?;

        let (ua, body) = match spec.crypto {
            Crypto::Weapi => (pick_user_agent(spec.ua), weapi(&json_text)),
            Crypto::Eapi => {
                // EAPI 的 url_path 用 service 写的逻辑路径(`spec.path`),
                // 不是改写后的实际 URL(spec §1.3 关键易错点)
                (pick_user_agent(spec.ua), eapi(spec.path, &json_text))
            }
            Crypto::Linuxapi => {
                // linuxapi 把 method+url+params 整个序列化进加密体
                let payload = serde_json::json!({
                    "method": "POST",
                    "url": format!("{BASE_URL}/api{}", strip_api_prefix(spec.path)),
                    "params": params,
                });
                (UA_LINUX, linuxapi(&payload.to_string()))
            }
        };

        let req = Request::post(&url)
            .header("Cookie", "os=pc; appver=2.7.1.198277; __remember_me=true")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Connection", "keep-alive")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Host", "music.163.com")
            .header("Referer", BASE_URL)
            .header("User-Agent", ua)
            .body(body)
            .map_err(|e| anyhow!("build request: {e}"))?;

        let mut resp = self
            .client
            .send_async(req)
            .await
            .map_err(|e| anyhow!("send: {e}"))?;
        let bytes = resp.bytes().await.map_err(|e| anyhow!("read body: {e}"))?;

        decode_response(bytes)
    }
}

/// 把 service 写的 `/weapi/...` / `/api/...` / `/eapi/...` 前缀剥成 `/...`,
/// 用于 linuxapi 拼装内部 url 字段。
fn strip_api_prefix(path: &str) -> &str {
    for prefix in ["/weapi", "/eapi", "/api"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return rest;
        }
    }
    path
}
