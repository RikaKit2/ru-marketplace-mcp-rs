use serde::{Deserialize, Serialize};

use crate::connector::MarketplaceConnector;
use crate::domain::{ProductOffer, ScraperError};

/// Verification outcome when comparing an original offer against live marketplace data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OfferVerificationResult {
    /// True if price is unchanged and stock status has not changed.
    pub matched: bool,
    /// Difference between current price and expected price (current_price - expected_price).
    pub price_diff: i64,
    /// Whether the stock status has changed between offer and live card.
    pub stock_status_changed: bool,
    /// Live price in RUB as reported by the marketplace card.
    pub current_price: u64,
    /// Expected price in RUB as recorded in the original offer.
    pub expected_price: u64,
    /// Human-readable explanation of verification outcome and any discrepancies.
    pub details: String,
}

impl OfferVerificationResult {
    /// Helper to verify an offer using the provided connector.
    pub async fn verify(
        connector: &dyn MarketplaceConnector,
        original_offer: &ProductOffer,
    ) -> Result<Self, ScraperError> {
        verify_offer(connector, original_offer).await
    }
}

/// Verify an offer against live marketplace data by fetching its product card.
///
/// Fetches the product card via `connector.get_card()` using `product_id` if available,
/// or `url` as fallback, and compares current price and stock status against `original_offer`.
pub async fn verify_offer(
    connector: &dyn MarketplaceConnector,
    original_offer: &ProductOffer,
) -> Result<OfferVerificationResult, ScraperError> {
    let item_id = match &original_offer.product_id {
        Some(id) if !id.trim().is_empty() => id.as_str(),
        _ => original_offer.url.as_str(),
    };

    let card = connector.get_card(item_id).await?;

    let expected_price = original_offer.price_tier.price();
    let current_price = card.price_tier.price();
    let price_diff = current_price as i64 - expected_price as i64;
    let stock_status_changed = card.stock != original_offer.stock;
    let matched = price_diff == 0 && !stock_status_changed;

    let mut reasons = Vec::new();
    if price_diff != 0 {
        reasons.push(format!(
            "price changed from {expected_price} RUB to {current_price} RUB (diff: {price_diff:+})"
        ));
    }
    if stock_status_changed {
        reasons.push(format!(
            "stock status changed from {:?} to {:?}",
            original_offer.stock, card.stock
        ));
    }

    let details = if reasons.is_empty() {
        format!(
            "Offer verified: price ({} RUB) and stock status ({:?}) match",
            current_price, card.stock
        )
    } else {
        format!("Offer changed: {}", reasons.join("; "))
    };

    Ok(OfferVerificationResult {
        matched,
        price_diff,
        stock_status_changed,
        current_price,
        expected_price,
        details,
    })
}
