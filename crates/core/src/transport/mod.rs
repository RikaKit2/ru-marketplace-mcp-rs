pub mod fingerprint;
pub mod retry;

pub use retry::send_with_transient_retry;

pub use fingerprint::{
    ACCEPT_LANG_RU, DEFAULT_DESKTOP_UA, SEC_CH_UA, SEC_CH_UA_MOBILE, SEC_CH_UA_PLATFORM,
    apply_browser_fingerprint, build_browser_headers,
};
