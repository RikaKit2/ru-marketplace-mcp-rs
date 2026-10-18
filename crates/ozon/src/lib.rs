pub mod connector;
pub mod parser;

pub use connector::{OZON_COMPOSER_URL, OzonConnector};
pub use parser::{
    OzonAspectItem, OzonReviewItem, OzonReviewScore, OzonReviewsResponse, extract_sku,
    parse_card_json, parse_composer_json, parse_price_str, parse_reviews_json,
};
