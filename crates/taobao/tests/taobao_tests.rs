use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, ProductOffer, ScraperError};
use ru_marketplace_taobao::{
    TaobaoConnector, canonical_item_url, canonical_search_url, extract_item_id, parse_card_html,
    parse_cny_price, parse_sales_count, parse_search_html,
};

const SEARCH_GRID: &str = include_str!("fixtures/search_grid.html");
const SEARCH_GRID_LIVE: &str = include_str!("fixtures/search_grid_live.html");
const SEARCH_LOGIN_WALL_LIVE: &str = include_str!("fixtures/search_login_wall_live.html");
const ITEM_CARD: &str = include_str!("fixtures/item_card.html");
const ITEM_CARD_LIVE: &str = include_str!("fixtures/item_card_live.html");

#[test]
fn test_extract_item_id() {
    // Bare numeric string
    assert_eq!(
        extract_item_id("123456789012"),
        Some("123456789012".to_string())
    );

    // Full HTTPS URL with query params
    assert_eq!(
        extract_item_id("https://item.taobao.com/item.htm?id=123456789012&spm=a21n57.1"),
        Some("123456789012".to_string())
    );

    // Scheme-relative URL
    assert_eq!(
        extract_item_id("//item.taobao.com/item.htm?id=123456789012&spm=a21n57.1"),
        Some("123456789012".to_string())
    );

    // Query string fragment
    assert_eq!(
        extract_item_id("item.htm?id=123456789012"),
        Some("123456789012".to_string())
    );

    // Untrusted non-taobao domain
    assert_eq!(
        extract_item_id("https://malicious.com/item.htm?id=123456789012"),
        None
    );

    // Invalid length / non-numeric
    assert_eq!(extract_item_id("12345"), None);
    assert_eq!(extract_item_id("abcdefghijk"), None);
}

#[test]
fn test_canonical_urls() {
    assert_eq!(
        canonical_item_url("123456789012"),
        "https://item.taobao.com/item.htm?id=123456789012"
    );

    let search_url = canonical_search_url("手机", 1);
    assert!(search_url.starts_with("https://s.taobao.com/search?q="));

    let search_page2 = canonical_search_url("headphones", 2);
    assert!(search_page2.contains("page=2"));
}

#[test]
fn test_parse_cny_price() {
    assert_eq!(parse_cny_price("999¥"), Some(999.0));
    assert_eq!(parse_cny_price("¥7999"), Some(7999.0));
    assert_eq!(parse_cny_price("66.8"), Some(66.8));
    assert_eq!(parse_cny_price("¥ 123.45"), Some(123.45));
    assert_eq!(parse_cny_price("面议"), None);
    assert_eq!(parse_cny_price(""), None);
}

#[test]
fn test_parse_sales_count() {
    assert_eq!(parse_sales_count("2000+人付款"), Some(2000));
    assert_eq!(parse_sales_count("约售 12 件"), Some(12));
    assert_eq!(parse_sales_count("月销2000+"), Some(2000));
    assert_eq!(parse_sales_count("面议"), None);
}

#[test]
fn test_search_grid_modeled_fixture() {
    let items = parse_search_html(SEARCH_GRID, 10).expect("Should parse modeled search grid");
    assert_eq!(items.len(), 2);

    let first = &items[0];
    assert_eq!(first.item_id, "123456789012");
    assert_eq!(first.title, "Apple iPhone 16 Pro Max 全网通5G手机 官方标配");
    assert_eq!(first.price_cny, Some(999.0));
    assert_eq!(first.shop_name.as_deref(), Some("苹果官方旗舰店"));
    assert_eq!(first.sales.as_deref(), Some("2000+人付款"));
    assert_eq!(first.sales_count, Some(2000));
    assert_eq!(
        first.url,
        "https://item.taobao.com/item.htm?id=123456789012"
    );

    let second = &items[1];
    assert_eq!(second.item_id, "123456789013");
    assert_eq!(second.title, "二手手机 便宜出");
    // Hidden price must be None, never 0
    assert_eq!(second.price_cny, None);
    assert_eq!(second.shop_name.as_deref(), Some("二手优品店"));
    assert_eq!(second.sales.as_deref(), Some("约售 12 件"));
    assert_eq!(second.sales_count, Some(12));

    // Offer conversion check: CNY currency preserved without conversion
    let offer = ProductOffer::from(first.clone());
    assert_eq!(offer.marketplace, Marketplace::Taobao);
    assert_eq!(offer.currency, Currency::Cny);
    assert_eq!(offer.price_tier, PriceTier::Base(999));
    assert_eq!(offer.seller.as_deref(), Some("苹果官方旗舰店"));
    assert_eq!(offer.product_id.as_deref(), Some("123456789012"));
}

#[test]
fn test_search_grid_live_fixture() {
    let items = parse_search_html(SEARCH_GRID_LIVE, 10).expect("Should parse live search grid");
    assert!(!items.is_empty());

    let first = &items[0];
    assert_eq!(first.item_id, "1048008062319");
    assert!(first.title.contains("连衣裙"));
    assert_eq!(first.price_cny, Some(66.8));
    assert_eq!(first.sales.as_deref(), Some("200+人付款"));
    assert_eq!(first.sales_count, Some(200));
    assert_eq!(first.location.as_deref(), Some("广东 广州"));
    assert_eq!(
        first.url,
        "https://item.taobao.com/item.htm?id=1048008062319"
    );

    let second = &items[1];
    assert_eq!(second.item_id, "1060336448920");
    assert_eq!(second.price_cny, Some(112.0));
    assert_eq!(second.sales.as_deref(), Some("100+人付款"));

    let third = &items[2];
    assert_eq!(third.item_id, "709599481146");
    assert_eq!(third.price_cny, Some(16.9));
    assert_eq!(third.sales.as_deref(), Some("1000+人付款"));
}

#[test]
fn test_item_card_modeled_fixture() {
    let card = parse_card_html(ITEM_CARD, Some("123456789012")).expect("Should parse item card");

    assert_eq!(card.item_id, "123456789012");
    assert_eq!(
        card.title,
        "Apple iPhone 16 Pro Max 全网通5G手机 国行正品全新未拆封 官方标配"
    );
    assert_eq!(card.price_cny, Some(7999.0));
    assert_eq!(card.original_price_cny, Some(8999.0));
    assert_eq!(card.shop_name.as_deref(), Some("苹果官方旗舰店"));
    assert_eq!(card.sales.as_deref(), Some("2000+人付款"));
    assert_eq!(card.sales_count, Some(2000));
    assert_eq!(card.description_images, 3);
    assert_eq!(card.location.as_deref(), Some("广东深圳"));

    // Coupon price ¥7899 must be ignored
    assert_ne!(card.price_cny, Some(7899.0));
    assert_ne!(card.original_price_cny, Some(7899.0));

    // Offer conversion check
    let offer = ProductOffer::from(card);
    assert_eq!(offer.marketplace, Marketplace::Taobao);
    assert_eq!(offer.currency, Currency::Cny);
    assert_eq!(offer.price_tier, PriceTier::Base(7999));
    assert_eq!(offer.original_price, Some(8999));
}

#[test]
fn test_item_card_live_fixture() {
    let card = parse_card_html(ITEM_CARD_LIVE, Some("1048008062319"))
        .expect("Should parse live item card");

    assert_eq!(card.item_id, "1048008062319");
    assert!(card.title.contains("连衣裙"));
}

#[test]
fn test_search_login_wall_live_fixture_detected() {
    let result = parse_search_html(SEARCH_LOGIN_WALL_LIVE, 10);
    assert!(result.is_err(), "Live login wall must trigger an error");

    let err = result.unwrap_err();
    assert!(
        err.is_waf_blocked(),
        "Live login wall must be classified as WafBlocked"
    );
}

#[test]
fn test_titled_login_wall_detection() {
    let html =
        "<html><head><title>淘宝网 - 登录页面</title></head><body>Please login</body></html>";
    let search_res = parse_search_html(html, 10);
    assert!(search_res.is_err());
    assert!(search_res.unwrap_err().is_waf_blocked());

    let card_res = parse_card_html(html, Some("123456789012"));
    assert!(card_res.is_err());
    assert!(card_res.unwrap_err().is_waf_blocked());
}

#[test]
fn test_item_gone_404_detection() {
    let html =
        "<html><head><title>很抱歉，您查看的商品不存在</title></head><body>Gone</body></html>";
    let card_res = parse_card_html(html, Some("123456789012"));
    assert!(card_res.is_err());

    match card_res.unwrap_err() {
        ScraperError::NotFound { query } => {
            assert_eq!(query, "123456789012");
        }
        other => panic!("Expected NotFound, got: {other:?}"),
    }
}

#[test]
fn test_connector_marketplace_taobao() {
    let connector = TaobaoConnector::new();
    assert_eq!(connector.marketplace(), Marketplace::Taobao);
}
