use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};

use crate::session::SessionData;

/// Default User-Agent simulating modern Chrome desktop on Linux.
pub const DEFAULT_DESKTOP_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Client Hints sec-ch-ua header.
pub const SEC_CH_UA: &str =
    "\"Chromium\";v=\"133\", \"Google Chrome\";v=\"133\", \"Not-A.Brand\";v=\"99\"";

/// Client Hints sec-ch-ua-mobile header.
pub const SEC_CH_UA_MOBILE: &str = "?0";

/// Client Hints sec-ch-ua-platform header.
pub const SEC_CH_UA_PLATFORM: &str = "\"Linux\"";

/// Realistic Russian locale accept-language header.
pub const ACCEPT_LANG_RU: &str = "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7";

/// Construct realistic Chrome 133 Linux browser headers with Client Hints and cookies.
pub fn build_browser_headers(session: Option<&SessionData>) -> HeaderMap {
    let mut headers = HeaderMap::new();

    let ua = session
        .map(|s| s.user_agent.as_str())
        .unwrap_or(DEFAULT_DESKTOP_UA);

    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(ua).unwrap_or(HeaderValue::from_static(DEFAULT_DESKTOP_UA)),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static(ACCEPT_LANG_RU));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );

    headers.insert(
        HeaderName::from_static("sec-ch-ua"),
        HeaderValue::from_static(SEC_CH_UA),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-mobile"),
        HeaderValue::from_static(SEC_CH_UA_MOBILE),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-platform"),
        HeaderValue::from_static(SEC_CH_UA_PLATFORM),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-dest"),
        HeaderValue::from_static("empty"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-mode"),
        HeaderValue::from_static("cors"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-site"),
        HeaderValue::from_static("same-site"),
    );

    if let Some(s) = session
        && !s.cookies.is_empty()
        && let Ok(cookie_val) = HeaderValue::from_str(&s.cookies)
    {
        headers.insert(reqwest::header::COOKIE, cookie_val);
    }

    headers
}

/// Apply browser fingerprint headers and session cookies to a reqwest RequestBuilder.
pub fn apply_browser_fingerprint(
    builder: reqwest::RequestBuilder,
    session: Option<&SessionData>,
) -> reqwest::RequestBuilder {
    builder.headers(build_browser_headers(session))
}
