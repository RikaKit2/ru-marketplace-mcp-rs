use async_trait::async_trait;
use std::sync::Arc;

use ru_marketplace_mcp::connectors::MarketplaceConnector;
use ru_marketplace_mcp::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus, WafProtection,
};
use ru_marketplace_mcp::services::AggregatorService;

struct MockConnector {
    marketplace: Marketplace,
    offers: Vec<ProductOffer>,
    fail: bool,
}

#[async_trait]
impl MarketplaceConnector for MockConnector {
    fn marketplace(&self) -> Marketplace {
        self.marketplace
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<ProductOffer>, ScraperError> {
        if self.fail {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "Mock anti-bot wall".to_string(),
            });
        }
        Ok(self.offers.clone())
    }

    async fn get_card(&self, _item_id: &str) -> Result<ProductOffer, ScraperError> {
        self.offers
            .first()
            .cloned()
            .ok_or_else(|| ScraperError::NotFound {
                query: "mock".to_string(),
            })
    }
}

fn make_offer(mkt: Marketplace, title: &str, price: u64) -> ProductOffer {
    ProductOffer {
        marketplace: mkt,
        title: title.to_string(),
        url: format!("https://example.com/{mkt}"),
        price_tier: PriceTier::Base(price),
        original_price: None,
        currency: Currency::Rub,
        rating: Some(4.8),
        review_count: Some(100),
        stock: StockStatus::InStock(1),
        seller: Some("Store".to_string()),
        product_id: Some("1".to_string()),
    }
}

#[tokio::test]
async fn test_aggregator_ranking_and_spread() {
    let wb = Arc::new(MockConnector {
        marketplace: Marketplace::Wildberries,
        offers: vec![make_offer(Marketplace::Wildberries, "Offer B (WB)", 1500)],
        fail: false,
    });
    let ozon = Arc::new(MockConnector {
        marketplace: Marketplace::Ozon,
        offers: vec![make_offer(Marketplace::Ozon, "Offer A (Ozon)", 1000)],
        fail: false,
    });
    let dns = Arc::new(MockConnector {
        marketplace: Marketplace::Dns,
        offers: vec![make_offer(Marketplace::Dns, "Offer C (DNS)", 2000)],
        fail: false,
    });

    let aggregator = AggregatorService::new(vec![wb, ozon, dns]);
    let result = aggregator.compare_prices("test product", 5, None).await;

    assert_eq!(result.total_offers, 3);
    assert!(result.complete);
    assert_eq!(result.sources_ok.len(), 3);
    assert_eq!(result.sources_queried.len(), 3);

    // Offers must be sorted by price ascending: 1000, 1500, 2000
    assert_eq!(result.offers[0].price_tier.price(), 1000);
    assert_eq!(result.offers[0].marketplace, Marketplace::Ozon);
    assert_eq!(result.offers[1].price_tier.price(), 1500);
    assert_eq!(result.offers[1].marketplace, Marketplace::Wildberries);
    assert_eq!(result.offers[2].price_tier.price(), 2000);
    assert_eq!(result.offers[2].marketplace, Marketplace::Dns);

    // Cheapest offer
    let cheapest = result.cheapest.unwrap();
    assert_eq!(cheapest.price_tier.price(), 1000);
    assert_eq!(cheapest.marketplace, Marketplace::Ozon);

    // Price spread: 2000 - 1000 = 1000
    assert_eq!(result.price_spread_rub, Some(1000));
}

#[tokio::test]
async fn test_aggregator_partial_results_on_failure() {
    let wb = Arc::new(MockConnector {
        marketplace: Marketplace::Wildberries,
        offers: vec![make_offer(Marketplace::Wildberries, "Offer WB", 1200)],
        fail: false,
    });
    let failing_ozon = Arc::new(MockConnector {
        marketplace: Marketplace::Ozon,
        offers: vec![],
        fail: true,
    });

    let aggregator = AggregatorService::new(vec![wb, failing_ozon]);
    let result = aggregator.compare_prices("test product", 5, None).await;

    // Partial results: WB succeeded, Ozon failed
    assert_eq!(result.total_offers, 1);
    assert!(!result.complete);
    assert_eq!(result.sources_ok.len(), 1);
    assert_eq!(result.sources_queried.len(), 2);
    assert!(!result.warnings.is_empty());

    let ozon_outcome = result
        .source_outcomes
        .iter()
        .find(|o| o.source == Marketplace::Ozon)
        .unwrap();
    assert_eq!(ozon_outcome.status, "waf_blocked");
    assert_eq!(ozon_outcome.offers_returned, 0);
}

#[tokio::test]
async fn test_aggregator_filter_sources() {
    let wb = Arc::new(MockConnector {
        marketplace: Marketplace::Wildberries,
        offers: vec![make_offer(Marketplace::Wildberries, "Offer WB", 1200)],
        fail: false,
    });
    let ozon = Arc::new(MockConnector {
        marketplace: Marketplace::Ozon,
        offers: vec![make_offer(Marketplace::Ozon, "Offer Ozon", 1400)],
        fail: false,
    });

    let aggregator = AggregatorService::new(vec![wb, ozon]);
    let filter = [Marketplace::Wildberries];
    let result = aggregator
        .compare_prices("test product", 5, Some(&filter))
        .await;

    assert_eq!(result.total_offers, 1);
    assert_eq!(result.sources_queried, vec![Marketplace::Wildberries]);
    assert_eq!(result.offers[0].marketplace, Marketplace::Wildberries);
}
