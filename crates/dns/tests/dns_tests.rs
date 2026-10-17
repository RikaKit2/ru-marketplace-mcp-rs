use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier};
use ru_marketplace_dns::DnsConnector;

#[test]
fn test_dns_parse_catalog_data() {
    let mock_json = r#"[
        {
            "name": "6.1\" Смартфон Apple iPhone 15 128 ГБ черный",
            "url": "https://www.dns-shop.ru/product/12345/iphone-15/",
            "price": 74999,
            "sku": "54321"
        }
    ]"#;

    let offers = DnsConnector::parse_catalog_data(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);

    let offer = &offers[0];
    assert_eq!(offer.marketplace, Marketplace::Dns);
    assert_eq!(offer.title, "6.1\" Смартфон Apple iPhone 15 128 ГБ черный");
    assert_eq!(offer.price_tier, PriceTier::Base(74999));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.seller, Some("DNS".to_string()));
}
