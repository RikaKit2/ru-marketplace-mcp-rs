use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, ScraperError, StockStatus};
use ru_marketplace_wb::{
    BasketResolver, WbReviewItem, WbReviewsResponse, WbRootInfo, WbSellerInfo,
    WildberriesConnector, basket_host_for_vol, decode_mojibake, parse_products_json,
    parse_reviews_json, parse_root_info_json, parse_seller_info_json,
};

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
                                "product": 80000,
                                "total": 80000
                            }
                        }
                    ]
                }
            ]
        }
    }"#;

    let offers = parse_products_json(mock_json, 10).unwrap();
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
    let offers = parse_products_json(empty_json, 10).unwrap();
    assert!(offers.is_empty());

    let no_price_json = r#"{
        "data": {
            "products": [
                {
                    "id": 12345,
                    "name": "No Price Item",
                    "sizes": []
                }
            ]
        }
    }"#;
    let offers2 = parse_products_json(no_price_json, 10).unwrap();
    assert!(offers2.is_empty());
}

#[tokio::test]
async fn test_basket_resolver_calculations_and_cache() {
    let resolver = BasketResolver::default();
    assert_eq!(BasketResolver::calculate_vol(164379765), 1643);
    assert_eq!(BasketResolver::calculate_part(164379765), 164379);

    resolver
        .set_cached_host(1643, "basket-11.wbbasket.ru")
        .await;
    let base = resolver.basket_base(164379765).await.unwrap();
    assert_eq!(
        base,
        "https://basket-11.wbbasket.ru/vol1643/part164379/164379765/info"
    );

    let cached = resolver.get_cached_host(1643).await;
    assert_eq!(cached, Some("basket-11.wbbasket.ru".to_string()));
}

#[test]
fn test_wildberries_connector_creation() {
    use ru_marketplace_core::connector::MarketplaceConnector;
    let connector = WildberriesConnector::default();
    assert_eq!(connector.marketplace(), Marketplace::Wildberries);
    assert_eq!(std::sync::Arc::strong_count(connector.session_manager()), 1);
}

#[test]
fn test_basket_resolver_adaptive_ceiling() {
    let resolver = BasketResolver::default();
    assert_eq!(resolver.max_known_basket(), 60);
}

#[test]
fn test_basket_host_prediction() {
    assert_eq!(basket_host_for_vol(100), "basket-01.wbbasket.ru");
    assert_eq!(basket_host_for_vol(1643), "basket-11.wbbasket.ru");
    assert_eq!(basket_host_for_vol(5500), "basket-28.wbbasket.ru");
}

#[test]
fn test_decode_mojibake() {
    assert_eq!(decode_mojibake("Привет мир"), "Привет мир");
    let latin1_decoded: String = vec![0xD0u8, 0xA2, 0xD0, 0xB5, 0xD1, 0x81, 0xD1, 0x82]
        .into_iter()
        .map(|b| b as char)
        .collect();
    assert_eq!(decode_mojibake(&latin1_decoded), "Тест");
}

#[test]
fn test_wb_parse_root_info_json() {
    let mock_json = r#"{
        "imt_id": 1002173489,
        "subj_name": "Сетевые фильтры",
        "subj_root_name": "Электроника",
        "vendor_code": "DEF-5535",
        "description": "Надежный сетевой фильтр с защитой от перенапряжения",
        "options": [
            {"name": "Количество розеток", "value": "5"},
            {"name": "Длина кабеля", "value": "3 м"}
        ]
    }"#;

    let root = parse_root_info_json(
        mock_json,
        5535522,
        Some("basket-01.wbbasket.ru".to_string()),
    )
    .unwrap();
    assert_eq!(root.imt_id, 1002173489);
    assert_eq!(root.subj_name, "Сетевые фильтры");
    assert_eq!(root.subj_root_name, "Электроника");
    assert_eq!(root.vendor_code.as_deref(), Some("DEF-5535"));
    assert_eq!(
        root.description.as_deref(),
        Some("Надежный сетевой фильтр с защитой от перенапряжения")
    );
    assert_eq!(root.options.len(), 2);
    assert_eq!(root.host_used.as_deref(), Some("basket-01.wbbasket.ru"));
}

#[test]
fn test_wb_parse_reviews_json() {
    let mock_json = r#"{
        "feedbackCount": 1500,
        "valuation": "4.8",
        "feedbacks": [
            {
                "productValuation": 5,
                "text": "Отличный сетевой фильтр, работает без нареканий",
                "pros": "Длинный провод, кнопка с подсветкой",
                "cons": "Не обнаружено",
                "wbUserDetails": {
                    "name": "Алексей"
                },
                "createdDate": "2026-04-12T14:32:00Z"
            },
            {
                "productValuation": 4,
                "text": "Хороший, но цвет темнее чем на фото",
                "pros": "Качество сборки",
                "cons": "Цвет",
                "wbUserDetails": {
                    "name": "Елена"
                },
                "createdDate": "2026-04-10T09:15:00Z"
            }
        ]
    }"#;

    let res = parse_reviews_json(
        mock_json,
        1002173489,
        1,
        Some("feedbacks2.wb.ru".to_string()),
    )
    .unwrap();
    assert_eq!(res.imt_id, 1002173489);
    assert_eq!(res.feedback_count, Some(1500));
    assert_eq!(res.valuation.as_deref(), Some("4.8"));
    assert_eq!(res.feedbacks.len(), 2);
    assert_eq!(res.page, 1);
    assert_eq!(res.total_pages, 1);

    let first = &res.feedbacks[0];
    assert_eq!(first.rating, Some(5));
    assert_eq!(
        first.text,
        "Отличный сетевой фильтр, работает без нареканий"
    );
    assert_eq!(first.pros, "Длинный провод, кнопка с подсветкой");
    assert_eq!(first.cons, "Не обнаружено");
    assert_eq!(first.user, "Алексей");
    assert_eq!(first.date.as_deref(), Some("2026-04-12T14:32:00Z"));
}

#[test]
fn test_wb_parse_seller_info_json() {
    let mock_json = r#"{
        "supplierName": "ООО ДЕФЕНДЕР РУС",
        "supplierFullName": "ОБЩЕСТВО С ОГРАНИЧЕННОЙ ОТВЕТСТВЕННОСТЬЮ \"ДЕФЕНДЕР РУС\"",
        "inn": "7701987654",
        "ogrn": "1127746123456",
        "kpp": "770101001",
        "legalAddress": "127000, г. Москва, ул. Складочная, д. 1",
        "trademark": "Defender"
    }"#;

    let seller = parse_seller_info_json(
        mock_json,
        12345,
        Some("static-basket-01.wbbasket.ru".to_string()),
    )
    .unwrap();
    assert_eq!(seller.supplier_id, 12345);
    assert_eq!(seller.supplier_name, "ООО ДЕФЕНДЕР РУС");
    assert_eq!(
        seller.full_name.as_deref(),
        Some("ОБЩЕСТВО С ОГРАНИЧЕННОЙ ОТВЕТСТВЕННОСТЬЮ \"ДЕФЕНДЕР РУС\"")
    );
    assert_eq!(seller.inn.as_deref(), Some("7701987654"));
    assert_eq!(seller.ogrn.as_deref(), Some("1127746123456"));
    assert_eq!(
        seller.legal_address.as_deref(),
        Some("127000, г. Москва, ул. Складочная, д. 1")
    );
    assert_eq!(seller.trademark.as_deref(), Some("Defender"));
    assert_eq!(seller.kpp.as_deref(), Some("770101001"));
}

#[tokio::test]
async fn test_wb_get_cards_batch_validation() {
    let connector = WildberriesConnector::default();
    let empty_res = connector.get_cards_batch(&[]).await.unwrap();
    assert!(empty_res.is_empty());

    let ids: Vec<u64> = (1..=101).collect();
    let err = connector.get_cards_batch(&ids).await.unwrap_err();
    assert!(matches!(err, ScraperError::InvalidFilter(_)));
}
