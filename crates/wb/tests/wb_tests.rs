use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, StockStatus};
use ru_marketplace_wb::{BasketResolver, WildberriesConnector, parse_products_json};

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
                    "id": 99999,
                    "name": "Товар без цены",
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
    let nm = 164379765;
    assert_eq!(BasketResolver::calculate_vol(nm), 1643);
    assert_eq!(BasketResolver::calculate_part(nm), 164379);

    let resolver = BasketResolver::default();
    assert!(resolver.get_cached_host(1643).await.is_none());

    resolver
        .set_cached_host(1643, "basket-11.wbbasket.ru")
        .await;
    assert_eq!(
        resolver.get_cached_host(1643).await,
        Some("basket-11.wbbasket.ru".to_string())
    );

    let base = resolver.basket_base(nm).await.unwrap();
    assert_eq!(
        base,
        "https://basket-11.wbbasket.ru/vol1643/part164379/164379765/info"
    );
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
