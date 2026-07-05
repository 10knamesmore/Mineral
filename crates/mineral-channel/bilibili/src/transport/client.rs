//! isahc 客户端 + WBI keys 缓存 + 请求 dispatch。

use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{WrapErr, eyre};
use isahc::cookies::{Cookie, CookieJar};
use isahc::http::Uri;
use isahc::{AsyncReadResponseExt, HttpClient, Request, config::Configurable};
use serde_json::Value;

use crate::config::BilibiliConfig;
use crate::credential::StoredBilibiliAuth;
use crate::error::ApiCodeError;
use crate::sign::wbi::{extract_key, sign_with_wts};
use crate::transport::headers::{REFERER, UA};
use crate::wire::nav::NavData;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

/// nav 端点:取 WBI keys(guest 返回 -101 但仍带 `wbi_img`,故走 lax)。
const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";

/// 首页:冷启动 GET 一次拿 `buvid3` set-cookie 进 jar(搜索端点需要)。
const HOME_URL: &str = "https://www.bilibili.com";

/// WBI keys 缓存有效期(每日更替,取 <1h 保守 TTL)。
const WBI_TTL: Duration = Duration::from_secs(3600);

/// 签名失效(风控 / keys 过期)的业务 code——命中则刷新 keys 重签一次。
const CODE_WBI_EXPIRED: i64 = -352;

/// 一对 WBI 签名 key(从 nav 的图片 URL 文件名提取)。
#[derive(Clone)]
struct WbiKeys {
    /// `img_key`。
    img_key: String,

    /// `sub_key`。
    sub_key: String,
}

/// 带取回时刻的 WBI keys 缓存项。
struct CachedKeys {
    /// keys 本体。
    keys: WbiKeys,

    /// 取回时刻(判 TTL)。
    fetched_at: Instant,
}

/// HTTP 传输层:isahc 客户端 + WBI keys 缓存 + buvid3 冷启动标记。
pub struct Transport {
    /// 底层 isahc 客户端,持有连接池 + cookie jar。
    client: HttpClient,

    /// WBI keys 缓存(TTL 见 [`WBI_TTL`])。
    wbi: Mutex<Option<CachedKeys>>,

    /// buvid3 冷启动是否已完成(避免每次搜索都打首页)。
    buvid3_ready: AtomicBool,
}

impl Transport {
    /// 按配置构造(超时 / 并发 / 代理);启用 cookie jar。
    ///
    /// # Params:
    ///   - `config`: B站源配置(超时 / 代理 / 并发)
    ///
    /// # Return:
    ///   传输层实例;isahc 客户端构建失败时 `Err`。
    pub fn new(config: &BilibiliConfig) -> Result<Self> {
        let mut builder = HttpClient::builder()
            .timeout(Duration::from_secs(*config.timeout_secs()))
            .max_connections(*config.max_connections())
            .cookies();
        if let Some(p) = config.proxy().as_deref() {
            builder = builder.proxy(Some(p.parse().context("invalid proxy url")?));
        }
        let client = builder.build().context("build isahc client failed")?;
        Ok(Self {
            client,
            wbi: Mutex::new(None),
            buvid3_ready: AtomicBool::new(false),
        })
    }

    /// 用登录凭证构造:把 `SESSDATA` / `bili_jct` / `DedeUserID` 塞进 cookie jar,
    /// 解锁高码率取流 + 私密收藏夹。
    ///
    /// # Params:
    ///   - `config`: B站源配置
    ///   - `auth`: 登录凭证三件套
    ///
    /// # Return:
    ///   带登录态的传输层;cookie / 客户端构建失败时 `Err`。
    pub fn from_credential(config: &BilibiliConfig, auth: &StoredBilibiliAuth) -> Result<Self> {
        let jar = CookieJar::new();
        let url: Uri = HOME_URL
            .parse()
            .map_err(|e| eyre!("parse bilibili uri: {e}"))?;
        for (name, value) in [
            ("SESSDATA", auth.sessdata.as_str()),
            ("bili_jct", auth.bili_jct.as_str()),
            ("DedeUserID", auth.dede_user_id.as_str()),
        ] {
            // domain 不带前导点:isahc 的 domain_matches 对 `.bilibili.com` 判 host(www/api)
            // 均 false → jar.set 报 DomainMismatch。用 `bilibili.com`(host-only 语义)才能既通过
            // set 校验、又在后续对 api.bilibili.com 的请求上附带。
            let cookie = Cookie::builder(name, value)
                .domain("bilibili.com")
                .path("/")
                .build()
                .map_err(|e| eyre!("build cookie {name}: {e}"))?;
            jar.set(cookie, &url)
                .map_err(|e| eyre!("set cookie {name}: {e}"))?;
        }
        let mut builder = HttpClient::builder()
            .timeout(Duration::from_secs(*config.timeout_secs()))
            .max_connections(*config.max_connections())
            .cookies()
            .cookie_jar(jar);
        if let Some(p) = config.proxy().as_deref() {
            builder = builder.proxy(Some(p.parse().context("invalid proxy url")?));
        }
        let client = builder.build().context("build isahc client failed")?;
        Ok(Self {
            client,
            wbi: Mutex::new(None),
            buvid3_ready: AtomicBool::new(false),
        })
    }

    /// 暴露内部 cookie jar(二维码登录后从中读 `SESSDATA` / `bili_jct` / `DedeUserID`)。
    pub fn cookie_jar(&self) -> Option<&CookieJar> {
        self.client.cookie_jar()
    }

    /// 发一个 GET,返回**整个信封**(不校验 `code`)。
    ///
    /// 用于 nav——guest 请求返回 `code = -101` 但 `data.wbi_img` 仍在。
    async fn get_value(&self, url: &str) -> Result<Value> {
        let req = Request::get(url)
            .header("User-Agent", UA)
            .header("Referer", REFERER)
            .body(())
            .map_err(|e| eyre!("build request: {e}"))?;
        let mut resp = self
            .client
            .send_async(req)
            .await
            .map_err(|e| eyre!("send: {e}"))?;
        let bytes = resp.bytes().await.map_err(|e| eyre!("read body: {e}"))?;
        serde_json::from_slice(&bytes).context("parse json envelope")
    }

    /// 发一个 GET,解 `{code, message, data}` 信封:`code == 0` 返回 `data`,否则结构化
    /// [`ApiCodeError`](channel 边界 downcast 映射)。
    ///
    /// # Params:
    ///   - `url`: 完整请求 URL(含已签名 query)
    ///
    /// # Return:
    ///   信封的 `data` 字段(无则 `Null`)。
    pub async fn get_data(&self, url: &str) -> Result<Value> {
        decode_envelope(&self.get_value(url).await?)
    }

    /// WBI 签名 GET:确保 buvid3 → 取 keys 签名 → 请求;命中 `-352`(签名失效)刷新 keys 重签一次。
    ///
    /// # Params:
    ///   - `base_url`: 端点 URL(不含 query)
    ///   - `params`: 业务参数(内部追加 `wts` + `w_rid`)
    ///
    /// # Return:
    ///   信封的 `data` 字段。
    pub async fn get_signed(&self, base_url: &str, params: Vec<(&str, String)>) -> Result<Value> {
        self.ensure_buvid3().await?;
        match self.signed_once(base_url, params.clone()).await {
            Err(e) if is_wbi_expired(&e) => {
                self.invalidate_keys();
                self.signed_once(base_url, params).await
            }
            other => other,
        }
    }

    /// 单次签名请求(不重试)。
    async fn signed_once(&self, base_url: &str, params: Vec<(&str, String)>) -> Result<Value> {
        let keys = self.wbi_keys().await?;
        let query = sign_with_wts(params, &keys.img_key, &keys.sub_key, now_secs());
        self.get_data(&format!("{base_url}?{query}")).await
    }

    /// 取 WBI keys:缓存命中且未过期直接用,否则拉 nav 刷新。
    async fn wbi_keys(&self) -> Result<WbiKeys> {
        {
            let guard = self.wbi.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(c) = guard.as_ref()
                && c.fetched_at.elapsed() < WBI_TTL
            {
                return Ok(c.keys.clone());
            }
        }
        let keys = self.fetch_nav_keys().await?;
        {
            let mut guard = self.wbi.lock().unwrap_or_else(PoisonError::into_inner);
            *guard = Some(CachedKeys {
                keys: keys.clone(),
                fetched_at: Instant::now(),
            });
        }
        Ok(keys)
    }

    /// 作废 keys 缓存(签名失效时下次强制重取)。
    fn invalidate_keys(&self) {
        *self.wbi.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// 从 nav 拉 `img_key`/`sub_key`(走 lax,guest 的 `-101` 不当错误)。
    async fn fetch_nav_keys(&self) -> Result<WbiKeys> {
        let value = self.get_value(NAV_URL).await?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let nav: NavData = crate::wire::de::from_value(data).context("解析 nav.wbi_img")?;
        let img_key =
            extract_key(&nav.wbi_img.img_url).ok_or_else(|| eyre!("nav img_key 提取失败"))?;
        let sub_key =
            extract_key(&nav.wbi_img.sub_url).ok_or_else(|| eyre!("nav sub_key 提取失败"))?;
        Ok(WbiKeys { img_key, sub_key })
    }

    /// buvid3 冷启动:首次 GET 首页,让 set-cookie 的 buvid3 进 jar(搜索端点需要)。已完成则跳过。
    async fn ensure_buvid3(&self) -> Result<()> {
        if self.buvid3_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        let req = Request::get(HOME_URL)
            .header("User-Agent", UA)
            .header("Referer", REFERER)
            .body(())
            .map_err(|e| eyre!("build request: {e}"))?;
        let mut resp = self
            .client
            .send_async(req)
            .await
            .map_err(|e| eyre!("send: {e}"))?;
        // 只为拿 set-cookie;body 排空即可。
        let _ = resp.bytes().await;
        self.buvid3_ready.store(true, Ordering::Release);
        Ok(())
    }
}

/// 解 `{code, message, data}` 信封:`code == 0` 返回 `data`,否则结构化 [`ApiCodeError`]。
fn decode_envelope(v: &Value) -> Result<Value> {
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        return Err(color_eyre::Report::new(ApiCodeError { code, message }));
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// 该错误是否是 WBI 签名失效(`-352`)。
fn is_wbi_expired(e: &color_eyre::Report) -> bool {
    e.downcast_ref::<ApiCodeError>()
        .is_some_and(|a| a.code == CODE_WBI_EXPIRED)
}

/// 当前 unix 秒(取不到时钟时退 0,签名仍能发出、由服务端判过期)。
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::decode_envelope;
    use crate::error::ApiCodeError;

    /// from_credential 把三件套 cookie 塞进 jar,且能被 `www` **和** `api` 两个子域取回。
    ///
    /// 回归:曾用 `.domain(".bilibili.com")`(前导点)→ isahc `domain_matches` 对 `www`/`api`
    /// 主机均判 false → `jar.set` 报 DomainMismatch → from_credential 直接 Err → 登录成功后
    /// daemon 启动构造带凭证 channel 失败、整个 B站源被跳过(比 guest 还差)。
    #[test]
    fn from_credential_cookies_attach_to_bilibili_subdomains() -> color_eyre::Result<()> {
        use isahc::http::Uri;

        use crate::config::BilibiliConfig;
        use crate::credential::StoredBilibiliAuth;

        let cfg = BilibiliConfig::builder()
            .max_connections(0)
            .proxy(None)
            .timeout_secs(30)
            .build();
        let auth = StoredBilibiliAuth {
            sessdata: "SESS_X".to_owned(),
            bili_jct: "JCT_Y".to_owned(),
            dede_user_id: "42".to_owned(),
        };
        let t = super::Transport::from_credential(&cfg, &auth)?;
        let jar = t
            .cookie_jar()
            .ok_or_else(|| color_eyre::eyre::eyre!("登录态 transport 应带 cookie jar"))?;
        for host in ["https://www.bilibili.com", "https://api.bilibili.com"] {
            let uri: Uri = host
                .parse()
                .map_err(|e| color_eyre::eyre::eyre!("parse {host}: {e}"))?;
            let cookie = jar
                .get_by_name(&uri, "SESSDATA")
                .ok_or_else(|| color_eyre::eyre::eyre!("{host} 应能取回 SESSDATA cookie"))?;
            assert_eq!(cookie.value(), "SESS_X", "{host} 的 SESSDATA 值应正确");
        }
        Ok(())
    }

    /// `code == 0` 时解出 `data`。
    #[test]
    fn envelope_ok_returns_data() -> color_eyre::Result<()> {
        let v = serde_json::json!({ "code": 0, "message": "0", "data": { "x": 1 } });
        let data = decode_envelope(&v)?;
        assert_eq!(data, serde_json::json!({ "x": 1 }));
        Ok(())
    }

    /// `code != 0` 时结构化成 [`ApiCodeError`](含 code/message),供 channel 边界 downcast。
    #[test]
    fn envelope_nonzero_is_structured_error() -> color_eyre::Result<()> {
        let v = serde_json::json!({ "code": -352, "message": "风控校验失败" });
        let Err(err) = decode_envelope(&v) else {
            return Err(color_eyre::eyre::eyre!("非 0 code 应报错"));
        };
        let api = err
            .downcast_ref::<ApiCodeError>()
            .ok_or_else(|| color_eyre::eyre::eyre!("应能 downcast 回 ApiCodeError"))?;
        assert_eq!(api.code, -352);
        assert_eq!(api.message, "风控校验失败");
        Ok(())
    }
}
