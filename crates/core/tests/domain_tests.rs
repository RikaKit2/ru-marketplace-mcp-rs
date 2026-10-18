use ru_marketplace_core::domain::{
    CardType, Currency, JsonRpcRequest, JsonRpcResponse, Marketplace, McpToolRequest, PriceTier,
    ProductOffer, StockStatus, SubscriptionType, WafProtection,
};
use std::str::FromStr;

#[test]
fn test_marketplace_serde_and_from_str() {
    for &m in Marketplace::ALL {
        let json = serde_json::to_string(&m).unwrap();
        let de: Marketplace = serde_json::from_str(&json).unwrap();
        assert_eq!(m, de);

        let s = m.as_str();
        let parsed = Marketplace::from_str(s).unwrap();
        assert_eq!(m, parsed);
    }

    assert_eq!(
        Marketplace::from_str("wb").unwrap(),
        Marketplace::Wildberries
    );
    assert_eq!(Marketplace::from_str("ozon").unwrap(), Marketplace::Ozon);
    assert_eq!(
        Marketplace::from_str("yandex-market").unwrap(),
        Marketplace::YandexMarket
    );
    assert_eq!(Marketplace::from_str("dns_shop").unwrap(), Marketplace::Dns);
    assert!(Marketplace::from_str("unknown_market").is_err());
}

#[test]
fn test_card_type_serde() {
    let cards = [
        CardType::OzonCard,
        CardType::WbWallet,
        CardType::SberSpasibo,
        CardType::YandexPay,
    ];

    for card in cards {
        let json = serde_json::to_string(&card).unwrap();
        let de: CardType = serde_json::from_str(&json).unwrap();
        assert_eq!(card, de);
    }

    assert_eq!(CardType::from_str("ozon_card").unwrap(), CardType::OzonCard);
    assert_eq!(CardType::from_str("wb").unwrap(), CardType::WbWallet);
}

#[test]
fn test_subscription_type_serde() {
    let subs = [
        SubscriptionType::YandexPlus,
        SubscriptionType::SberPrime,
        SubscriptionType::OzonPremium,
    ];

    for sub in subs {
        let json = serde_json::to_string(&sub).unwrap();
        let de: SubscriptionType = serde_json::from_str(&json).unwrap();
        assert_eq!(sub, de);
    }
}

#[test]
fn test_currency_serde() {
    let currencies = [Currency::Rub, Currency::Cny, Currency::Usd];
    for c in currencies {
        let json = serde_json::to_string(&c).unwrap();
        let de: Currency = serde_json::from_str(&json).unwrap();
        assert_eq!(c, de);
    }

    assert_eq!(Currency::from_str("₽").unwrap(), Currency::Rub);
    assert_eq!(Currency::from_str("cny").unwrap(), Currency::Cny);
}

#[test]
fn test_price_tier_methods() {
    let base = PriceTier::Base(1500);
    assert_eq!(base.price(), 1500);
    assert_eq!(base.to_rubles_f64(), 1500.0);

    let kopecks_tier = PriceTier::from_kopecks(149950);
    assert_eq!(kopecks_tier.price(), 1500);

    let card_tier = PriceTier::card_from_kopecks(120000, CardType::WbWallet);
    assert_eq!(card_tier.price(), 1200);

    let sub_tier = PriceTier::subscription_from_kopecks(99900, SubscriptionType::YandexPlus);
    assert_eq!(sub_tier.price(), 999);

    let coupon_tier = PriceTier::CouponApplied {
        price: 800,
        code: "SALE20".to_string(),
    };
    assert_eq!(coupon_tier.price(), 800);
}

#[test]
fn test_stock_status() {
    let in_stock = StockStatus::InStock(5);
    assert!(in_stock.is_in_stock());
    assert_eq!(in_stock.quantity(), Some(5));

    let out_of_stock = StockStatus::OutOfStock;
    assert!(!out_of_stock.is_in_stock());
    assert_eq!(out_of_stock.quantity(), None);

    let pre_order = StockStatus::PreOrder;
    assert!(pre_order.is_in_stock());

    let unknown = StockStatus::Unknown;
    assert!(!unknown.is_in_stock());
}

#[test]
fn test_product_offer_ranking() {
    let offer_rub = ProductOffer {
        marketplace: Marketplace::Wildberries,
        title: "Test Product".to_string(),
        url: "https://wb.ru/1".to_string(),
        price_tier: PriceTier::Base(1999),
        original_price: Some(2500),
        currency: Currency::Rub,
        rating: Some(4.8),
        review_count: Some(150),
        stock: StockStatus::InStock(10),
        seller: Some("Official Store".to_string()),
        product_id: Some("1".to_string()),
    };

    assert_eq!(offer_rub.rank_price_rub(), Some(1999));

    let mut offer_cny = offer_rub.clone();
    offer_cny.currency = Currency::Cny;
    assert_eq!(offer_cny.rank_price_rub(), None);
}

#[test]
fn test_waf_protection_display() {
    assert_eq!(WafProtection::Wbaas.to_string(), "Wbaas (Wildberries)");
    assert_eq!(
        WafProtection::Kasada.to_string(),
        "Kasada / Qrator Anti-Bot"
    );
    assert_eq!(
        WafProtection::CloudflareTurnstile.to_string(),
        "Cloudflare Turnstile"
    );
}

#[test]
fn test_mcp_tool_request_serde() {
    let compare = McpToolRequest::ComparePrices {
        query: "macbook air m2".to_string(),
        limit: Some(10),
        filters: None,
    };

    let json = serde_json::to_string(&compare).unwrap();
    let de: McpToolRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(compare, de);

    let get_card = McpToolRequest::GetCard {
        marketplace: Marketplace::Wildberries,
        item_id: "12345678".to_string(),
    };
    let json2 = serde_json::to_string(&get_card).unwrap();
    let de2: McpToolRequest = serde_json::from_str(&json2).unwrap();
    assert_eq!(get_card, de2);
}

#[test]
fn test_json_rpc_messages() {
    let req_str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let req: JsonRpcRequest = serde_json::from_str(req_str).unwrap();
    assert_eq!(req.method, "initialize");
    assert_eq!(req.id, Some(serde_json::json!(1)));

    let resp = JsonRpcResponse::success(
        Some(serde_json::json!(1)),
        serde_json::json!({"status":"ok"}),
    );
    let resp_str = serde_json::to_string(&resp).unwrap();
    assert!(resp_str.contains(r#""result":{"status":"ok"}"#));

    let err_resp = JsonRpcResponse::error(Some(serde_json::json!(1)), -32601, "Method not found");
    let err_str = serde_json::to_string(&err_resp).unwrap();
    assert!(err_str.contains("Method not found"));
}

#[test]
fn test_search_filter_options_serde() {
    use ru_marketplace_core::domain::{SearchFilterOptions, SortOrder};
    use std::str::FromStr;

    assert_eq!(
        SortOrder::from_str("price_asc").unwrap(),
        SortOrder::PriceAsc
    );
    assert_eq!(
        SortOrder::from_str("price_desc").unwrap(),
        SortOrder::PriceDesc
    );
    assert_eq!(SortOrder::from_str("rating").unwrap(), SortOrder::Rating);
    assert_eq!(SortOrder::from_str("reviews").unwrap(), SortOrder::Reviews);
    assert_eq!(
        SortOrder::from_str("discount").unwrap(),
        SortOrder::Discount
    );

    let opts = SearchFilterOptions {
        min_price: Some(10000),
        max_price: Some(25000),
        sort: Some(SortOrder::PriceAsc),
    };

    let json = serde_json::to_string(&opts).unwrap();
    assert!(json.contains("10000"));
    assert!(json.contains("price_asc"));

    let de: SearchFilterOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(opts, de);
}

#[test]
fn test_marketplace_from_url() {
    assert_eq!(
        Marketplace::from_url("https://www.wildberries.ru/catalog/123/detail.aspx"),
        Some(Marketplace::Wildberries)
    );
    assert_eq!(
        Marketplace::from_url("https://basket-01.wbbasket.ru/vol1/part1/123/info/ru/card.json"),
        Some(Marketplace::Wildberries)
    );
    assert_eq!(
        Marketplace::from_url("https://www.ozon.ru/product/456/"),
        Some(Marketplace::Ozon)
    );
    assert_eq!(
        Marketplace::from_url("https://market.yandex.ru/product--test/789"),
        Some(Marketplace::YandexMarket)
    );
    assert_eq!(
        Marketplace::from_url("https://www.dns-shop.ru/product/abc/"),
        Some(Marketplace::Dns)
    );
    assert_eq!(
        Marketplace::from_url("https://www.citilink.ru/product/xyz/"),
        Some(Marketplace::Citilink)
    );
    assert_eq!(
        Marketplace::from_url("https://unknown-shop.com/product/1"),
        None
    );
}

#[tokio::test]
async fn test_offer_verification_success_and_diff() {
    use async_trait::async_trait;
    use ru_marketplace_core::MarketplaceConnector;
    use ru_marketplace_core::domain::{
        Currency, Marketplace, PriceTier, ProductOffer, ScraperError, SearchFilterOptions,
        StockStatus, verify_offer,
    };

    struct TestConnector {
        card_to_return: Result<ProductOffer, String>,
    }

    #[async_trait]
    impl MarketplaceConnector for TestConnector {
        fn marketplace(&self) -> Marketplace {
            Marketplace::Wildberries
        }
        async fn search(
            &self,
            _query: &str,
            _limit: usize,
            _filters: &SearchFilterOptions,
        ) -> Result<Vec<ProductOffer>, ScraperError> {
            Ok(vec![])
        }
        async fn get_card(&self, _item_id: &str) -> Result<ProductOffer, ScraperError> {
            match &self.card_to_return {
                Ok(card) => Ok(card.clone()),
                Err(e) => Err(ScraperError::Parse(e.clone())),
            }
        }
    }

    let original = ProductOffer {
        marketplace: Marketplace::Wildberries,
        title: "Test Item".to_string(),
        url: "https://www.wildberries.ru/catalog/123/detail.aspx".to_string(),
        price_tier: PriceTier::Base(1000),
        original_price: Some(1500),
        currency: Currency::Rub,
        rating: Some(4.8),
        review_count: Some(100),
        stock: StockStatus::InStock(10),
        seller: Some("Official Store".to_string()),
        product_id: Some("123".to_string()),
    };

    // Case 1: Exact match
    let conn1 = TestConnector {
        card_to_return: Ok(original.clone()),
    };
    let res1 = verify_offer(&conn1, &original).await.unwrap();
    assert!(res1.matched);
    assert_eq!(res1.price_diff, 0);
    assert!(!res1.stock_status_changed);
    assert_eq!(res1.current_price, 1000);
    assert_eq!(res1.expected_price, 1000);

    // Case 2: Price changed + Stock changed
    let mut modified = original.clone();
    modified.price_tier = PriceTier::Base(1200);
    modified.stock = StockStatus::OutOfStock;
    let conn2 = TestConnector {
        card_to_return: Ok(modified),
    };
    let res2 = verify_offer(&conn2, &original).await.unwrap();
    assert!(!res2.matched);
    assert_eq!(res2.price_diff, 200);
    assert!(res2.stock_status_changed);
    assert_eq!(res2.current_price, 1200);
    assert_eq!(res2.expected_price, 1000);
    assert!(
        res2.details
            .contains("price changed from 1000 RUB to 1200 RUB")
    );
    assert!(res2.details.contains("stock status changed"));
}
