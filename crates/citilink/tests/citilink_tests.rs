use ru_marketplace_citilink::CitilinkConnector;
use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier};

#[test]
fn test_citilink_parse_catalog_data() {
    let mock_json = r#"[
        {
            "name": "Смартфон Apple iPhone 15 128Gb, черный",
            "url": "https://www.citilink.ru/product/smartfon-apple-iphone-15-128gb-1987654/",
            "offers": {
                "price": 72990
            },
            "sku": "1987654"
        }
    ]"#;

    let offers = CitilinkConnector::parse_catalog_data(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);

    let offer = &offers[0];
    assert_eq!(offer.marketplace, Marketplace::Citilink);
    assert_eq!(offer.title, "Смартфон Apple iPhone 15 128Gb, черный");
    assert_eq!(offer.price_tier, PriceTier::Base(72990));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.seller, Some("Citilink".to_string()));
}
