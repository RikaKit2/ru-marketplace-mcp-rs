use ru_marketplace_core::domain::{CardType, Currency, Marketplace, PriceTier, StockStatus};
use ru_marketplace_yandex::{
    ParseStatus, YandexConnector, looks_like_captcha, parse_card, parse_search,
};

const SEARCH_IPHONE: &str = include_str!("fixtures/search_iphone.html");
const CARD_WASHER: &str = include_str!("fixtures/card_washer.html");
const SEARCH_TELEFON_ZONE: &str = include_str!("fixtures/search_telefon_zone.html");
const CARD_EMPTY_SHELL: &str = include_str!("fixtures/card_empty_shell.html");
const SEARCH_EMPTY: &str = include_str!("fixtures/search_empty.html");
const CARD_NO_RATING: &str = include_str!("fixtures/card_no_rating.html");

#[test]
fn test_search_iphone_parsing() {
    let result = parse_search(SEARCH_IPHONE);

    assert_eq!(result.status, ParseStatus::Ok);
    assert_eq!(result.total, Some(688));
    assert_eq!(result.page, Some(1));
    assert_eq!(result.items.len(), 3);

    let first = &result.items[0];
    assert_eq!(first.product_id.as_deref(), Some("1912483624"));
    assert_eq!(first.brand.as_deref(), Some("Apple"));
    assert_eq!(first.seller.as_deref(), Some("Созвон"));
    assert_eq!(first.price_rub, Some(57128.0));
    assert_eq!(first.price_with_plus, Some(55414.0));
    assert_eq!(first.price_old_rub, Some(74711.0));
    assert!(first.url.starts_with("https://"));
    assert_eq!(first.source, "ssr");
    assert!(first.title.contains("iPhone 15"));

    let second = &result.items[1];
    assert_eq!(second.seller.as_deref(), Some("Original Gadget"));
    assert_eq!(second.price_rub, Some(56415.0));

    let third = &result.items[2];
    assert_eq!(third.product_id.as_deref(), Some("5884422069"));
    assert_eq!(third.seller.as_deref(), Some("Дамоф"));
    assert_eq!(third.price_rub, Some(50117.0));
}

#[test]
fn test_card_washer_parsing() {
    let card = parse_card(CARD_WASHER);

    assert_eq!(card.status, ParseStatus::Ok);
    assert_eq!(card.product_id, "763970960");
    assert_eq!(card.sku_id.as_deref(), Some("5737172259"));
    assert!(card.title.contains("Tuvio"));
    assert_eq!(card.price_rub, Some(19377.0));
    assert_eq!(card.price_with_plus, Some(18796.0));
    assert_eq!(card.price_before_discount_rub, Some(26185.0));
    assert_eq!(card.rating, Some(4.8));
    assert_eq!(card.rating_count, Some(544));
    assert_eq!(card.review_count, Some(209));
    assert_eq!(card.reviews.len(), 2);

    let review = &card.reviews[0];
    assert_eq!(review.author, "Иван И.");
    assert_eq!(review.rating, Some(5));
    assert_eq!(review.date, "20 июля");
    assert!(review.pros.contains("Стирка"));
    assert!(review.votes_up >= 0);
}

#[test]
fn test_search_zone_snippets() {
    let result = parse_search(SEARCH_TELEFON_ZONE);

    assert_eq!(result.status, ParseStatus::OkZone);
    assert_eq!(result.items.len(), 3);

    let first = &result.items[0];
    assert_eq!(first.product_id.as_deref(), Some("4315891968"));
    assert_eq!(first.sku_id.as_deref(), Some("103796664836"));
    assert_eq!(first.price_rub, Some(11329.0));
    assert_eq!(first.price_with_plus, Some(11102.0));
    assert_eq!(first.price_old_rub, Some(12450.0));
    assert_eq!(first.source, "zone");
}

#[test]
fn test_card_empty_shell() {
    let card = parse_card(CARD_EMPTY_SHELL);

    assert_eq!(card.status, ParseStatus::EmptyProductShell);
    assert_eq!(card.product_id, "4315891968");
    assert!(card.title.is_empty());
    assert!(card.price_rub.is_none());
}

#[test]
fn test_search_empty_query() {
    let result = parse_search(SEARCH_EMPTY);

    assert_eq!(result.status, ParseStatus::Empty);
    assert!(result.items.is_empty());
}

#[test]
fn test_card_no_rating() {
    let card = parse_card(CARD_NO_RATING);

    assert_eq!(card.status, ParseStatus::Ok);
    assert!(!card.title.is_empty());
    assert!(card.price_rub.is_some());
    assert!(card.rating.is_none());
    assert!(card.rating_stars.is_empty());
    assert!(card.reviews.is_empty());
}

#[test]
fn test_captcha_detection() {
    assert!(looks_like_captcha(
        r#"<html><body><div id="SmartCaptcha">Please solve</div></body></html>"#
    ));
    assert!(looks_like_captcha(
        r#"<html><body><form action="/showcaptcha">Captcha</form></body></html>"#
    ));
    assert!(looks_like_captcha(
        r#"<html><body><div class="checkbox_captcha"></div></body></html>"#
    ));
    // A regular empty captchaService placeholder should NOT trigger captcha detection
    assert!(!looks_like_captcha(
        r#"<html><body><div id="/content/captchaService"></div></body></html>"#
    ));
}

#[test]
fn test_yandex_connector_search_mapping() {
    let offers = YandexConnector::parse_search_page(SEARCH_IPHONE, 10).unwrap();
    assert_eq!(offers.len(), 3);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::YandexMarket);
    assert!(first.title.contains("iPhone 15"));
    assert_eq!(
        first.price_tier,
        PriceTier::MarketplaceCard {
            price: 55414,
            card: CardType::YandexPay,
        }
    );
    assert_eq!(first.original_price, Some(74711));
    assert_eq!(first.currency, Currency::Rub);
    assert_eq!(first.seller.as_deref(), Some("Созвон"));
    assert_eq!(first.stock, StockStatus::InStock(1));
}

#[test]
fn test_yandex_connector_card_mapping() {
    let offer = YandexConnector::parse_card_page(CARD_WASHER).unwrap();
    assert_eq!(offer.marketplace, Marketplace::YandexMarket);
    assert!(offer.title.contains("Tuvio"));
    assert_eq!(
        offer.price_tier,
        PriceTier::MarketplaceCard {
            price: 18796,
            card: CardType::YandexPay,
        }
    );
    assert_eq!(offer.original_price, Some(26185));
    assert_eq!(offer.rating, Some(4.8));
    assert_eq!(offer.review_count, Some(209));
    assert_eq!(offer.product_id.as_deref(), Some("763970960"));
}
