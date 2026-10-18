use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, ScraperError, StockStatus};
use ru_marketplace_wb::{
    BasketResolver, WbCategoryNode, WildberriesConnector, basket_host_for_vol, decode_mojibake,
    find_menu_subtree, parse_categories, parse_products_json, parse_reviews_json,
    parse_root_info_json, parse_seller_info_json,
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

const MOCK_WB_MENU_JSON: &str = r#"[
  {
    "id": 8126,
    "name": "Электроника",
    "url": "/catalog/elektronika",
    "shard": "electronic58",
    "query": "cat=8126",
    "childs": [
      {
        "id": 9845,
        "name": "Смартфоны и телефоны",
        "url": "/catalog/elektronika/smartfony-i-telefony",
        "shard": "electronic58",
        "query": "cat=9845",
        "childs": [
          {
            "id": 1234,
            "name": "Смартфоны",
            "url": "/catalog/elektronika/smartfony",
            "shard": "electronic58",
            "query": "cat=1234"
          }
        ]
      },
      {
        "id": 9846,
        "name": "Ноутбуки",
        "url": "/catalog/elektronika/noutbuki",
        "shard": "electronic58",
        "query": "cat=9846"
      }
    ]
  },
  {
    "id": 8200,
    "name": "Бытовая техника",
    "url": "/catalog/bytovaya-tehnika",
    "shard": "blackhole",
    "query": "cat=8200",
    "childs": [
      {
        "id": 8201,
        "name": "Пылесосы",
        "url": "/catalog/bytovaya-tehnika/pylesosy",
        "shard": "appliances2",
        "query": "cat=8201"
      }
    ]
  }
]"#;

#[test]
fn test_wb_find_menu_subtree() {
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    // Search by exact name
    let found = find_menu_subtree(&menu, "Электроника").expect("should find by name");
    assert_eq!(found.get("id").and_then(|v| v.as_u64()), Some(8126));

    // Search by nested name
    let found_sub = find_menu_subtree(&menu, "Ноутбуки").expect("should find nested category");
    assert_eq!(found_sub.get("id").and_then(|v| v.as_u64()), Some(9846));

    // Search by url
    let found_url = find_menu_subtree(&menu, "/catalog/elektronika").expect("should find by url");
    assert_eq!(found_url.get("id").and_then(|v| v.as_u64()), Some(8126));

    // Search by url slug
    let found_slug = find_menu_subtree(&menu, "noutbuki").expect("should find by slug");
    assert_eq!(found_slug.get("id").and_then(|v| v.as_u64()), Some(9846));

    // Search by numeric ID
    let found_id = find_menu_subtree(&menu, "9845").expect("should find by id");
    assert_eq!(
        found_id.get("name").and_then(|v| v.as_str()),
        Some("Смартфоны и телефоны")
    );

    // Not found
    assert!(find_menu_subtree(&menu, "Несуществующая категория").is_none());
}

#[test]
fn test_wb_parse_categories_top_level() {
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    let res =
        parse_categories(&menu, "top", 1, Some("static-basket-01.wbbasket.ru".into())).unwrap();
    assert_eq!(res.root, "top");
    assert_eq!(res.max_depth, 1);
    assert_eq!(res.items.len(), 2);
    assert_eq!(res.total_returned, 2);
    assert!(!res.truncated);
    assert_eq!(
        res.host_used.as_deref(),
        Some("static-basket-01.wbbasket.ru")
    );

    let first = &res.items[0];
    assert_eq!(first.id, Some(8126));
    assert_eq!(first.name, "Электроника");
    assert_eq!(first.url, "/catalog/elektronika");
    assert_eq!(first.shard.as_deref(), Some("electronic58"));
    assert_eq!(first.query.as_deref(), Some("cat=8126"));
    assert_eq!(first.depth, 0);
    assert_eq!(first.children_count, 2);
    // With max_depth = 1, children are not expanded
    assert!(first.childs.is_empty());
    assert!(first.children().is_empty());
}

#[test]
fn test_wb_parse_categories_depth_expansion() {
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    let res = parse_categories(&menu, "top", 2, None).unwrap();
    assert_eq!(res.max_depth, 2);
    assert_eq!(res.items.len(), 2);
    // 2 top-level + 2 children of first + 1 child of second = 5 total nodes returned
    assert_eq!(res.total_returned, 5);

    let first = &res.items[0];
    assert_eq!(first.childs.len(), 2);
    assert_eq!(first.childs[0].name, "Смартфоны и телефоны");
    assert_eq!(first.childs[0].depth, 1);
    assert_eq!(first.childs[0].children_count, 1);
    // Grandchildren not expanded at max_depth = 2
    assert!(first.childs[0].childs.is_empty());

    assert_eq!(first.childs[1].name, "Ноутбуки");
    assert_eq!(first.childs[1].depth, 1);
}

#[test]
fn test_wb_parse_categories_subtree() {
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    let res = parse_categories(&menu, "Электроника", 1, None).unwrap();
    assert_eq!(res.root, "Электроника");
    assert_eq!(res.items.len(), 2);
    assert_eq!(res.items[0].name, "Смартфоны и телефоны");
    assert_eq!(res.items[1].name, "Ноутбуки");
}

#[test]
fn test_wb_parse_categories_not_found_and_empty() {
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    let err_not_found = parse_categories(&menu, "UnknownCategory", 1, None).unwrap_err();
    assert!(matches!(err_not_found, ScraperError::NotFound { .. }));

    let err_empty = parse_categories(&[], "top", 1, None).unwrap_err();
    assert!(matches!(err_empty, ScraperError::Parse(_)));
}

#[tokio::test]
async fn test_wb_category_products_validation_and_blackhole() {
    let connector = WildberriesConnector::default();

    // Blackhole shard must return ScraperError::InvalidFilter
    let err_blackhole = connector
        .get_category_products("blackhole", "cat=8200", 1, None)
        .await
        .unwrap_err();
    match err_blackhole {
        ScraperError::InvalidFilter(msg) => {
            assert!(msg.contains("blackhole"));
            assert!(msg.contains("только навигационной"));
        }
        other => panic!("expected InvalidFilter for blackhole, got: {other:?}"),
    }

    // Empty shard validation
    let err_empty_shard = connector
        .get_category_products("", "cat=8200", 1, None)
        .await
        .unwrap_err();
    assert!(matches!(err_empty_shard, ScraperError::InvalidFilter(_)));

    // Empty query validation
    let err_empty_query = connector
        .get_category_products("appliances2", "", 1, None)
        .await
        .unwrap_err();
    assert!(matches!(err_empty_query, ScraperError::InvalidFilter(_)));
}

#[tokio::test]
async fn test_wb_menu_caching_in_connector() {
    let connector = WildberriesConnector::default();
    let menu: Vec<serde_json::Value> = serde_json::from_str(MOCK_WB_MENU_JSON).unwrap();

    // Pre-populate cache
    connector
        .set_menu_cache(menu, "mock-static-host.wbbasket.ru".to_string())
        .await;

    // Retrieve categories via connector without any network request
    let res = connector.get_categories("top", 1).await.unwrap();
    assert_eq!(res.root, "top");
    assert_eq!(res.items.len(), 2);
    assert_eq!(
        res.host_used.as_deref(),
        Some("mock-static-host.wbbasket.ru")
    );

    // Retrieve subtree via connector
    let sub = connector.get_categories("Ноутбуки", 1).await.unwrap();
    assert_eq!(sub.root, "Ноутбуки");
    assert_eq!(sub.items.len(), 0); // leaf category
}

#[test]
fn test_wb_category_node_serde() {
    let node = WbCategoryNode {
        id: Some(42),
        name: "Test Cat".to_string(),
        url: "/catalog/test".to_string(),
        shard: Some("shard1".to_string()),
        query: Some("cat=42".to_string()),
        depth: 0,
        children_count: 1,
        childs: vec![WbCategoryNode {
            id: Some(43),
            name: "Child Cat".to_string(),
            url: "/catalog/test/child".to_string(),
            shard: Some("shard1".to_string()),
            query: Some("cat=43".to_string()),
            depth: 1,
            children_count: 0,
            childs: Vec::new(),
        }],
    };

    let json = serde_json::to_string(&node).unwrap();
    let deserialized: WbCategoryNode = serde_json::from_str(&json).unwrap();
    assert_eq!(node, deserialized);
    assert_eq!(deserialized.children().len(), 1);
}
