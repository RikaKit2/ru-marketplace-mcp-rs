use ru_marketplace_core::domain::{CardType, Currency, Marketplace, PriceTier, StockStatus};
use ru_marketplace_ozon::{parse_card_json, parse_composer_json, parse_price_str};

#[test]
fn test_ozon_parse_price_str() {
    assert_eq!(parse_price_str("1 499 ₽"), Some(1499));
    assert_eq!(parse_price_str("12 490 ₽"), Some(12490));
    assert_eq!(parse_price_str("99"), Some(99));
    assert_eq!(parse_price_str("abc"), None);
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
                                "type": "textAtom",
                                "id": "name",
                                "textAtom": {
                                    "text": "Смартфон Apple iPhone 15 128GB"
                                }
                            },
                            {
                                "type": "priceV2",
                                "priceV2": {
                                    "price": "69 990 ₽",
                                    "originalPrice": "89 990 ₽",
                                    "theme": { "textColor": "ozon_card" }
                                }
                            },
                            {
                                "type": "labelList",
                                "labelList": {
                                    "items": [
                                        { "title": "4.9" }
                                    ]
                                }
                            }
                        ]
                    }
                ]
            }
        }
    }"#;

    let offers = parse_composer_json(mock_json, 10).unwrap();
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
    assert_eq!(offer.stock, StockStatus::InStock(1));
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

    let offer = parse_card_json(mock_json, "3087570418").unwrap();
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

    let res = parse_composer_json(incident_json, 10);
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

    let res = parse_card_json(incident_json, "12345");
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
                                "type": "textAtom",
                                "id": "name",
                                "textAtom": {
                                    "text": "Альтернативный товар"
                                }
                            },
                            {
                                "type": "price",
                                "price": {
                                    "price": "1 990 ₽"
                                }
                            }
                        ]
                    }
                ]
            }
        }
    }"#;

    let offers = parse_composer_json(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Альтернативный товар");
    assert_eq!(offers[0].price_tier, PriceTier::Base(1990));
}
