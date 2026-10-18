use ru_marketplace_aliexpress::{
    AliExpressConnector, AliExpressProduct, card_url, extract_item_id, is_empty_shell, is_punish,
    parse_card_page, parse_meta_description, parse_search_page, search_url,
};
use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, SearchFilterOptions, SortOrder, StockStatus,
};

const SEARCH_GRID_HTML: &str = include_str!("fixtures/search_grid.html");
const CARD_HTML: &str = include_str!("fixtures/card.html");

#[test]
fn test_parse_search_grid_fixture() {
    let products = parse_search_page(SEARCH_GRID_HTML, 10).expect("failed to parse search grid");
    assert_eq!(
        products.len(),
        4,
        "expected exactly 4 products from search grid fixture"
    );

    // 1. Realme Watch 5 (with -58% discount and strikethrough price)
    let realme = products
        .iter()
        .find(|p| p.item_id == "1005010003103368")
        .expect("Realme Watch 5 tile not found");
    assert_eq!(realme.title, "Realme Watch 5 смарт-часы");
    assert_eq!(realme.price_rub, Some(2939.0));
    assert_eq!(realme.old_price_rub, Some(6967.0));
    assert_eq!(realme.discount_percent, Some(58));
    assert_eq!(realme.rating, Some(4.7));
    assert_eq!(realme.orders_count, Some(2334));
    assert_eq!(realme.sku_id, Some("12000050856992499".to_string()));
    assert!(
        realme
            .image_url
            .as_ref()
            .map_or(false, |url| url.contains("ae-pic")),
        "expected valid product image preview"
    );

    // 2. realme Watch S5 white/gray (with coupon price)
    let s5_white = products
        .iter()
        .find(|p| p.item_id == "1005012078506501")
        .expect("realme Watch S5 white/gray not found");
    assert_eq!(s5_white.title, "realme Watch S5 белые/серые");
    assert_eq!(s5_white.price_rub, Some(5959.0));
    assert_eq!(s5_white.coupon_price_rub, Some(5700.0));
    assert_eq!(s5_white.rating, Some(5.0));
    assert_eq!(s5_white.orders_count, Some(42));

    // 3. realme Watch S5 (108 orders)
    let s5_108 = products
        .iter()
        .find(|p| p.item_id == "1005012061152213")
        .expect("realme Watch S5 108 orders not found");
    assert_eq!(s5_108.title, "realme Watch S5 умные часы");
    assert_eq!(s5_108.price_rub, Some(5800.0));
    assert_eq!(s5_108.coupon_price_rub, Some(5196.0));
    assert_eq!(s5_108.rating, Some(4.6));
    assert_eq!(s5_108.orders_count, Some(108));

    // 4. realme Watch S5 (190 orders)
    let s5_190 = products
        .iter()
        .find(|p| p.item_id == "1005012078314868")
        .expect("realme Watch S5 190 orders not found");
    assert_eq!(s5_190.title, "realme Watch S5 умные часы");
    assert_eq!(s5_190.price_rub, Some(5579.0));
    assert_eq!(s5_190.coupon_price_rub, Some(5320.0));
    assert_eq!(s5_190.rating, Some(4.9));
    assert_eq!(s5_190.orders_count, Some(190));
}

#[test]
fn test_product_offer_conversion() {
    let products = parse_search_page(SEARCH_GRID_HTML, 10).unwrap();
    let realme = products
        .into_iter()
        .find(|p| p.item_id == "1005010003103368")
        .unwrap();

    let offer: ProductOffer = realme.into();
    assert_eq!(offer.marketplace, Marketplace::AliExpress);
    assert_eq!(offer.title, "Realme Watch 5 смарт-часы");
    assert_eq!(offer.price_tier, PriceTier::Base(2939));
    assert_eq!(offer.original_price, Some(6967));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.rating, Some(4.7));
    assert_eq!(offer.review_count, Some(2334));
    assert_eq!(offer.stock, StockStatus::InStock(1));
    assert_eq!(offer.seller, Some("AliExpress".to_string()));
    assert_eq!(offer.product_id, Some("1005010003103368".to_string()));
}

#[test]
fn test_coupon_price_tier_conversion() {
    let products = parse_search_page(SEARCH_GRID_HTML, 10).unwrap();
    let s5 = products
        .into_iter()
        .find(|p| p.item_id == "1005012078506501")
        .unwrap();

    let offer: ProductOffer = s5.into();
    assert_eq!(
        offer.price_tier,
        PriceTier::CouponApplied {
            price: 5700,
            code: "aliexpress_coupon".to_string(),
        }
    );
}

#[test]
fn test_parse_card_fixture() {
    let card =
        parse_card_page(CARD_HTML, "1005010003103368").expect("failed to parse card fixture");

    assert_eq!(card.item_id, "1005010003103368");
    assert_eq!(card.title, "Realme Watch 5 смарт-часы");
    assert_eq!(card.orders_count, Some(2334));
    assert_eq!(card.rating, Some(4.7));
    assert_eq!(card.url, "https://aliexpress.ru/item/1005010003103368.html");
    assert!(card.in_stock);
    assert!(
        card.description
            .as_ref()
            .map_or(false, |d| d.contains("Realme Watch 5")),
        "description should contain product title"
    );

    let offer: ProductOffer = card.into();
    assert_eq!(offer.marketplace, Marketplace::AliExpress);
    assert_eq!(offer.title, "Realme Watch 5 смарт-часы");
    assert_eq!(offer.rating, Some(4.7));
    assert_eq!(offer.review_count, Some(2334));
    assert_eq!(offer.stock, StockStatus::InStock(1));
}

#[test]
fn test_extract_item_id_validation() {
    // Valid bare 9-16 digit IDs
    assert_eq!(
        extract_item_id("1005010003103368"),
        Some("1005010003103368".to_string())
    );
    assert_eq!(
        extract_item_id("  1005010003103368  "),
        Some("1005010003103368".to_string())
    );
    assert_eq!(extract_item_id("123456789"), Some("123456789".to_string()));

    // Valid URLs
    assert_eq!(
        extract_item_id("https://aliexpress.ru/item/1005010003103368.html"),
        Some("1005010003103368".to_string())
    );
    assert_eq!(
        extract_item_id("https://aliexpress.ru/item/1005010003103368.html?sku_id=123"),
        Some("1005010003103368".to_string())
    );
    assert_eq!(
        extract_item_id("https://m.aliexpress.ru/item/1005010003103368.html"),
        Some("1005010003103368".to_string())
    );
    assert_eq!(
        extract_item_id("/item/1005010003103368.html"),
        Some("1005010003103368".to_string())
    );

    // Invalid inputs
    assert_eq!(extract_item_id(""), None);
    assert_eq!(extract_item_id("   "), None);
    assert_eq!(extract_item_id("12345"), None); // Too short (<9 digits)
    assert_eq!(extract_item_id("12345678901234567"), None); // Too long (>16 digits)
    assert_eq!(
        extract_item_id("https://malicious.com/item/1005010003103368.html"),
        None
    );
    assert_eq!(
        extract_item_id("//aliexpress.ru/item/1005010003103368.html"),
        None
    );
}

#[test]
fn test_anti_bot_detection() {
    assert!(is_punish(
        "<html><body>punish?x5secdata=12345</body></html>"
    ));
    assert!(is_punish(
        "<html><body>Пожалуйста, пройдите проверку</body></html>"
    ));
    assert!(is_punish("<html><body>Sec-Firewall block</body></html>"));
    assert!(!is_punish("<html><body>Каталог товаров</body></html>"));

    assert!(is_empty_shell(
        "<html><body><script>window.__AER_DATA__={};</script></body></html>"
    ));
    assert!(is_empty_shell(
        "<html><body><div id=\"root\"></div></body></html>"
    ));
    assert!(!is_empty_shell(SEARCH_GRID_HTML));
}

#[test]
fn test_search_and_card_url_helpers() {
    let s_url = search_url("часы", 1);
    assert!(s_url.starts_with("https://aliexpress.ru/wholesale?SearchText="));
    assert!(!s_url.contains("&page="));

    let s_url_p2 = search_url("часы", 2);
    assert!(s_url_p2.contains("&page=2"));

    let c_url = card_url("1005010003103368");
    assert_eq!(c_url, "https://aliexpress.ru/item/1005010003103368.html");
}

#[test]
fn test_meta_description_parsing() {
    let desc = "Realme Watch 5 смарт-часы. ➜ 2334 заказов. ✓ Хорошие отзывы покупателей. ⭐ Рейтинг- 4.7 ⚡ Мы ускорили доставку!";
    let (orders, rating) = parse_meta_description(desc);
    assert_eq!(orders, Some(2334));
    assert_eq!(rating, Some(4.7));
}

#[test]
fn test_connector_instantiation() {
    let connector = AliExpressConnector::new();
    assert_eq!(
        ru_marketplace_core::connector::MarketplaceConnector::marketplace(&connector),
        Marketplace::AliExpress
    );
}
