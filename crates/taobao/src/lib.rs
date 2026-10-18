pub mod connector;
pub mod models;
pub mod parser;

pub use connector::TaobaoConnector;
pub use models::{
    TaobaoCardDetails, TaobaoExtractedCardPayload, TaobaoExtractedSearchPayload, TaobaoSearchItem,
};
pub use parser::{
    TAOBAO_ITEM_BASE, TAOBAO_SEARCH_BASE, canonical_item_url, canonical_search_url,
    check_login_wall_or_challenge, extract_item_id, parse_card_html, parse_cny_price,
    parse_sales_count, parse_search_html,
};
