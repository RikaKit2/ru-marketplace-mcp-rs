use ru_marketplace_mcp::domain::{
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
