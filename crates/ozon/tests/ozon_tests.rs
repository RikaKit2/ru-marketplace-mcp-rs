use ru_marketplace_core::domain::{CardType, Currency, Marketplace, PriceTier, StockStatus};
use ru_marketplace_ozon::{
    OzonAspectItem, OzonReviewItem, OzonReviewScore, OzonReviewsResponse, extract_sku,
    parse_card_json, parse_composer_json, parse_price_str, parse_reviews_json,
};

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

#[test]
fn test_ozon_parse_composer_v3_and_merged_widgets() {
    let mock_json = r#"{
        "widgetStates": {
            "searchResultsV3-39281-default-1": "{\"items\": [{\"sku\": \"3087570418\", \"action\": {\"link\": \"/product/3087570418/\"}, \"mainState\": [{\"type\": \"textAtom\", \"textAtom\": {\"text\": \"i5-14600K\"}}, {\"type\": \"priceV3\", \"priceV3\": {\"price\": [{\"text\": \"27 719 ₽\", \"theme\": \"ozon_card\"}]}}]}]}"
        }
    }"#;

    let offers = parse_composer_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 1);
    let offer = &offers[0];
    assert_eq!(offer.title, "i5-14600K");
    assert_eq!(offer.product_id, Some("3087570418".to_string()));
    assert_eq!(
        offer.price_tier,
        PriceTier::MarketplaceCard {
            price: 27719,
            card: CardType::OzonCard
        }
    );
}

#[test]
fn test_ozon_parse_composer_mega_paginator_nested_products() {
    let mock_json = r#"{
        "widgetStates": {
            "megaPaginator-12345-default-1": "{\"products\": [{\"id\": 987654321, \"action\": {\"link\": \"/product/987654321/\"}, \"mainState\": [{\"type\": \"textAtom\", \"textAtom\": {\"text\": \"Ryzen 7 7800X3D\"}}, {\"type\": \"price\", \"price\": {\"price\": [{\"text\": \"41 490 ₽\"}]}}]}]}"
        }
    }"#;

    let offers = parse_composer_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 1);
    let offer = &offers[0];
    assert_eq!(offer.title, "Ryzen 7 7800X3D");
    assert_eq!(offer.price_tier, PriceTier::Base(41490));
}

#[test]
fn test_ozon_extract_sku() {
    assert_eq!(extract_sku("145892019"), Some("145892019".to_string()));
    assert_eq!(
        extract_sku("/product/145892019/"),
        Some("145892019".to_string())
    );
    assert_eq!(
        extract_sku("/product/smartfon-apple-iphone-15-128gb-145892019/"),
        Some("145892019".to_string())
    );
    assert_eq!(
        extract_sku(
            "https://www.ozon.ru/product/smartfon-apple-iphone-15-128gb-145892019/?sh=test"
        ),
        Some("145892019".to_string())
    );
    assert_eq!(
        extract_sku("https://www.ozon.ru/product/145892019/reviews/"),
        Some("145892019".to_string())
    );
    assert_eq!(extract_sku(""), None);
}

#[test]
fn test_ozon_parse_reviews_full() {
    let mock_json = r#"{
        "widgetStates": {
            "webReviewProductScore-12345-default-1": "{\"totalScore\": 4.8, \"reviewsCount\": 150, \"score\": [{\"title\": \"5 звезд\", \"value\": 120}, {\"title\": \"4 звезды\", \"value\": 20}, {\"title\": \"1 звезда\", \"value\": 10}]}",
            "webListReviews-12345-default-1": "{\"paging\": {\"total\": 150, \"nextButton\": \"?page=2&sort=published_at_desc\"}, \"products\": {\"145892019\": {\"name\": \"iPhone 15 128GB\"}, \"999999999\": {\"name\": \"iPhone 15 256GB\"}}, \"reviews\": [{\"uuid\": \"rev-1\", \"itemId\": 145892019, \"author\": {\"firstName\": \"Иван\", \"lastName\": \"Иванов\"}, \"publishedAt\": 1715000000, \"content\": {\"score\": 5, \"comment\": \"Отличный смартфон\", \"positive\": \"Экран и камера\", \"negative\": \"Нет\", \"photos\": [{\"url\": \"https://example.com/p1.jpg\"}], \"videos\": [{\"url\": \"https://example.com/v1.mp4\"}]}, \"usefulness\": {\"useful\": 15, \"unuseful\": 2}}, {\"uuid\": \"rev-2\", \"itemId\": 999999999, \"author\": {\"firstName\": \"Анна\"}, \"publishedAt\": \"2024-05-01T12:00:00Z\", \"content\": {\"score\": 4, \"comment\": {\"text\": \"Хороший телефон\"}, \"positive\": \"Быстрый\"}, \"usefulness\": {\"useful\": 3, \"unuseful\": 0}}]}",
            "webAspects-12345-default-1": "{\"aspects\": [{\"title\": \"Качество\", \"score\": 4.9, \"positiveCount\": 140, \"negativeCount\": 5, \"sentiment\": \"positive\"}, {\"title\": \"Доставка\", \"score\": 4.8, \"positiveCount\": 135, \"negativeCount\": 2}, {\"title\": \"Соответствие описанию\", \"score\": 5.0, \"positiveCount\": 148, \"negativeCount\": 0}]}"
        }
    }"#;

    let res = parse_reviews_json(mock_json, "145892019", 1, "published_at_desc").unwrap();
    assert_eq!(res.status, "success");
    assert_eq!(res.sku, "145892019");
    assert_eq!(res.page, 1);
    assert_eq!(res.sort, "published_at_desc");
    assert_eq!(res.rating_score, Some(4.8));
    assert_eq!(res.reviews_count, Some(150));
    assert_eq!(res.has_next, true);
    assert_eq!(res.returned, 2);
    assert_eq!(res.own_reviews, 1);

    // Score & distribution
    let score = res.score.as_ref().unwrap();
    assert_eq!(score.rating_score, Some(4.8));
    assert_eq!(score.reviews_count, Some(150));
    assert_eq!(score.distribution.get(&5), Some(&120));
    assert_eq!(score.distribution.get(&4), Some(&20));
    assert_eq!(score.distribution.get(&1), Some(&10));

    // Reviews
    assert_eq!(res.reviews.len(), 2);
    let r1 = &res.reviews[0];
    assert_eq!(r1.id, Some("rev-1".to_string()));
    assert_eq!(r1.item_id, Some(145892019));
    assert_eq!(r1.author, Some("Иван Иванов".to_string()));
    assert_eq!(r1.score, Some(5));
    assert_eq!(r1.comment, Some("Отличный смартфон".to_string()));
    assert_eq!(r1.text, Some("Отличный смартфон".to_string()));
    assert_eq!(r1.positive, Some("Экран и камера".to_string()));
    assert_eq!(r1.negative, Some("Нет".to_string()));
    assert_eq!(r1.useful, Some(15));
    assert_eq!(r1.unuseful, Some(2));
    assert_eq!(r1.likes, Some(15));
    assert_eq!(r1.dislikes, Some(2));
    assert_eq!(r1.photos, 1);
    assert_eq!(r1.photo_urls, vec!["https://example.com/p1.jpg"]);
    assert_eq!(r1.videos, 1);
    assert_eq!(r1.video_urls, vec!["https://example.com/v1.mp4"]);
    assert!(r1.date.is_some());

    let r2 = &res.reviews[1];
    assert_eq!(r2.id, Some("rev-2".to_string()));
    assert_eq!(r2.item_id, Some(999999999));
    assert_eq!(r2.author, Some("Анна".to_string()));
    assert_eq!(r2.score, Some(4));
    assert_eq!(r2.comment, Some("Хороший телефон".to_string()));
    assert_eq!(r2.positive, Some("Быстрый".to_string()));
    assert_eq!(r2.negative, None);
    assert_eq!(r2.useful, Some(3));
    assert_eq!(r2.unuseful, Some(0));
    assert_eq!(r2.photos, 0);

    // Aspects
    assert_eq!(res.aspects.len(), 3);
    let a1 = &res.aspects[0];
    assert_eq!(a1.name, "Качество");
    assert_eq!(a1.score, Some(4.9));
    assert_eq!(a1.positive_count, Some(140));
    assert_eq!(a1.negative_count, Some(5));
    assert_eq!(a1.sentiment, Some("positive".to_string()));

    let a2 = &res.aspects[1];
    assert_eq!(a2.name, "Доставка");
    assert_eq!(a2.score, Some(4.8));

    let a3 = &res.aspects[2];
    assert_eq!(a3.name, "Соответствие описанию");
    assert_eq!(a3.score, Some(5.0));

    // Pool variants
    assert_eq!(
        res.pool_variants.get("145892019"),
        Some(&"iPhone 15 128GB".to_string())
    );
    assert_eq!(
        res.pool_variants.get("999999999"),
        Some(&"iPhone 15 256GB".to_string())
    );
}

#[test]
fn test_ozon_parse_reviews_empty_widget_states() {
    let mock_json = r#"{"widgetStates": {}}"#;
    let res = parse_reviews_json(mock_json, "12345", 1, "published_at_desc").unwrap();
    assert_eq!(res.status, "success");
    assert_eq!(res.returned, 0);
    assert_eq!(res.reviews.len(), 0);
    assert_eq!(res.aspects.len(), 0);
    assert_eq!(res.score, None);
}

#[test]
fn test_ozon_parse_reviews_incident_error() {
    let incident_json = r#"{
        "incidentId": "sec_incident_777",
        "widgetStates": {}
    }"#;
    let res = parse_reviews_json(incident_json, "12345", 1, "published_at_desc");
    assert!(res.is_err());
    assert!(res.unwrap_err().is_waf_blocked());
}
