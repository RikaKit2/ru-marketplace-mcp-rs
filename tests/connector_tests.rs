use ru_marketplace_mcp::connectors::{
    CitilinkConnector, DnsConnector, OzonConnector, WildberriesConnector,
};
use ru_marketplace_mcp::domain::{CardType, Currency, Marketplace, PriceTier, StockStatus};

#[test]
fn test_wb_parse_v9_json() {
    let mock_json = r#"{
        "data": {
            "products": [
                {
                    "id": 164379765,
                    "name": "Футляр для наушников",
                    "brand": "Apple",
                    "supplier": "WB Retail",
                    "reviewRating": 4.9,
                    "feedbacks": 3200,
                    "totalQuantity": 42,
                    "sizes": [
                        {
                            "price": {
                                "basic": 250000,
                                "product": 199900,
                                "total": 199900
                            }
                        }
                    ]
                },
                {
                    "id": 1345073040,
                    "name": "Чехол силиконовый",
                    "brand": "Spigen",
                    "supplier": "Spigen Official",
                    "reviewRating": 4.7,
                    "feedbacks": 150,
                    "totalQuantity": 0,
                    "sizes": [
                        {
                            "price": {
                                "basic": 100000,
                                "product": 80000
                            }
                        }
                    ]
                }
            ]
        }
    }"#;

    let offers = WildberriesConnector::parse_products_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 2);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::Wildberries);
    assert_eq!(first.title, "Футляр для наушников");
    assert_eq!(first.product_id, Some("164379765".to_string()));
    assert_eq!(
        first.url,
        "https://www.wildberries.ru/catalog/164379765/detail.aspx"
    );
    assert_eq!(first.price_tier, PriceTier::Base(1999));
    assert_eq!(first.original_price, Some(2500));
    assert_eq!(first.currency, Currency::Rub);
    assert_eq!(first.rating, Some(4.9));
    assert_eq!(first.review_count, Some(3200));
    assert_eq!(first.stock, StockStatus::InStock(42));
    assert_eq!(first.seller, Some("WB Retail".to_string()));

    let second = &offers[1];
    assert_eq!(second.stock, StockStatus::OutOfStock);
    assert_eq!(second.price_tier, PriceTier::Base(800));
}

#[test]
fn test_wb_parse_empty_or_no_price() {
    let empty_json = r#"{"data": {"products": []}}"#;
    let offers = WildberriesConnector::parse_products_json(empty_json, 10).unwrap();
    assert!(offers.is_empty());

    // Product with no sizes/price should be skipped
    let no_price_json = r#"{
        "data": {
            "products": [
                {
                    "id": 99999,
                    "name": "Товар без цены",
                    "sizes": []
                }
            ]
        }
    }"#;
    let offers2 = WildberriesConnector::parse_products_json(no_price_json, 10).unwrap();
    assert!(offers2.is_empty());
}

#[test]
fn test_ozon_parse_price_str() {
    assert_eq!(OzonConnector::parse_price_str("1 499 ₽"), Some(1499));
    assert_eq!(OzonConnector::parse_price_str("12 490 ₽"), Some(12490));
    assert_eq!(OzonConnector::parse_price_str("99"), Some(99));
    assert_eq!(OzonConnector::parse_price_str("abc"), None);
}

#[test]
fn test_ozon_parse_composer_json() {
    let mock_json = r#"{
        "widgetStates": {
            "tileGridDesktop-123456-default-1": {
                "items": [
                    {
                        "sku": "145892019",
                        "action": {
                            "link": "/product/smartfon-apple-iphone-15-128gb-145892019/?sh=test"
                        },
                        "mainState": [
                            {
                                "type": "textDS",
                                "id": "name",
                                "textDS": {
                                    "text": "Смартфон Apple iPhone 15 128GB"
                                }
                            },
                            {
                                "type": "priceV2",
                                "priceV2": {
                                    "price": [
                                        { "text": "69 990 ₽" },
                                        { "text": "89 990 ₽" }
                                    ],
                                    "ozonCardTag": true
                                }
                            },
                            {
                                "type": "labelList",
                                "labelList": {
                                    "items": [
                                        { "title": "4.9 • 450 отзывов" },
                                        { "title": "В наличии" }
                                    ]
                                }
                            }
                        ]
                    }
                ]
            }
        }
    }"#;

    let offers = OzonConnector::parse_composer_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 1);

    let offer = &offers[0];
    assert_eq!(offer.marketplace, Marketplace::Ozon);
    assert_eq!(offer.title, "Смартфон Apple iPhone 15 128GB");
    assert_eq!(
        offer.url,
        "https://www.ozon.ru/product/smartfon-apple-iphone-15-128gb-145892019/"
    );
    assert_eq!(
        offer.price_tier,
        PriceTier::MarketplaceCard {
            price: 69990,
            card: CardType::OzonCard
        }
    );
    assert_eq!(offer.original_price, Some(89990));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.rating, Some(4.9));
    assert_eq!(offer.review_count, Some(450));
    assert_eq!(offer.stock, StockStatus::InStock(1));
}

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

#[test]
fn test_ozon_parse_card_json() {
    let mock_json = r#"{
        "widgetStates": {
            "webProductHeading-123-default-1": {
                "title": "Процессор Intel Core i5-14600K LGA 1700 OEM"
            },
            "webPrice-456-default-1": {
                "isAvailable": true,
                "price": "27 999 ₽",
                "cardPrice": "27 719 ₽",
                "originalPrice": "35 000 ₽"
            },
            "webReviewProductScore-789-default-1": {
                "totalScore": 4.9,
                "reviewsCount": 61
            },
            "webBrand-101-default-1": {
                "content": {
                    "title": {
                        "text": [
                            {
                                "type": "link",
                                "content": "Intel"
                            }
                        ]
                    }
                }
            }
        }
    }"#;

    let offer = OzonConnector::parse_card_json(mock_json, "3087570418").unwrap();
    assert_eq!(offer.marketplace, Marketplace::Ozon);
    assert_eq!(offer.title, "Процессор Intel Core i5-14600K LGA 1700 OEM");
    assert_eq!(offer.product_id, Some("3087570418".to_string()));
    assert_eq!(
        offer.price_tier,
        PriceTier::MarketplaceCard {
            price: 27719,
            card: CardType::OzonCard
        }
    );
    assert_eq!(offer.original_price, Some(35000));
    assert_eq!(offer.rating, Some(4.9));
    assert_eq!(offer.review_count, Some(61));
    assert_eq!(offer.seller, Some("Intel".to_string()));
    assert_eq!(offer.stock, StockStatus::InStock(1));
}

#[test]
fn test_ozon_parse_composer_incident_error() {
    let incident_json = r#"{
        "incidentId": "fab_12345_test",
        "supportURL": "https://www.ozon.ru/complaint/support/?incident=fab_12345_test"
    }"#;

    let res = OzonConnector::parse_composer_json(incident_json, 10);
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.is_waf_blocked());
}

#[test]
fn test_ozon_parse_card_incident_error() {
    let incident_json = r#"{
        "incidentId": "fab_99999_test",
        "supportURL": "https://www.ozon.ru/complaint/support/?incident=fab_99999_test"
    }"#;

    let res = OzonConnector::parse_card_json(incident_json, "12345");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.is_waf_blocked());
}

#[test]
fn test_ozon_parse_composer_alternative_widget() {
    let mock_json = r#"{
        "widgetStates": {
            "searchResultsV2-333-default-1": {
                "items": [
                    {
                        "sku": "987654321",
                        "action": {
                            "link": "/product/alternative-test-item-987654321/"
                        },
                        "mainState": [
                            {
                                "type": "textDS",
                                "id": "name",
                                "textDS": {
                                    "text": "Альтернативный товар"
                                }
                            },
                            {
                                "type": "priceV2",
                                "priceV2": {
                                    "price": [
                                        { "text": "1 990 ₽" }
                                    ],
                                    "ozonCardTag": false
                                }
                            }
                        ]
                    }
                ]
            }
        }
    }"#;

    let offers = OzonConnector::parse_composer_json(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Альтернативный товар");
    assert_eq!(offers[0].price_tier, PriceTier::Base(1990));
}
