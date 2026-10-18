use serde::{Deserialize, Serialize};

use super::marketplaces::{CardType, Currency, Marketplace, StockStatus, SubscriptionType};

/// Price tier model capturing retail pricing, loyalty card discounts, and subscriptions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceTier {
    Base(u64),
    MarketplaceCard {
        price: u64,
        card: CardType,
    },
    LoyaltySubscription {
        price: u64,
        subscription: SubscriptionType,
    },
    CouponApplied {
        price: u64,
        code: String,
    },
}

impl PriceTier {
    /// Return the numerical price value (in main currency units, e.g. Rubles).
    pub fn price(&self) -> u64 {
        match self {
            Self::Base(p) => *p,
            Self::MarketplaceCard { price, .. } => *price,
            Self::LoyaltySubscription { price, .. } => *price,
            Self::CouponApplied { price, .. } => *price,
        }
    }

    /// Construct a base price from integer kopecks (rounds to nearest ruble).
    pub fn from_kopecks(kopecks: u64) -> Self {
        Self::Base((kopecks + 50) / 100)
    }

    /// Construct a marketplace card price tier from integer kopecks.
    pub fn card_from_kopecks(kopecks: u64, card: CardType) -> Self {
        Self::MarketplaceCard {
            price: (kopecks + 50) / 100,
            card,
        }
    }

    /// Construct a loyalty subscription price tier from integer kopecks.
    pub fn subscription_from_kopecks(kopecks: u64, subscription: SubscriptionType) -> Self {
        Self::LoyaltySubscription {
            price: (kopecks + 50) / 100,
            subscription,
        }
    }

    /// Return price as f64 rubles.
    pub fn to_rubles_f64(&self) -> f64 {
        self.price() as f64
    }
}

/// A normalized product offer returned by a marketplace connector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductOffer {
    /// Marketplace the offer originated from.
    pub marketplace: Marketplace,
    /// Product title / name.
    pub title: String,
    /// Direct link to the product card.
    pub url: String,
    /// Effective price tier (everyday, card, subscription, coupon).
    pub price_tier: PriceTier,
    /// Original / strikethrough price before discount, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_price: Option<u64>,
    /// Currency of the offer.
    pub currency: Currency,
    /// Rating on a 1.0 to 5.0 scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Number of customer reviews / feedbacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_count: Option<u32>,
    /// Stock status and available quantity.
    pub stock: StockStatus,
    /// Seller or supplier name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seller: Option<String>,
    /// Marketplace-native product or SKU identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
}

impl ProductOffer {
    /// Helper to get the base ruble price for sorting and ranking.
    pub fn rank_price_rub(&self) -> Option<u64> {
        if self.currency == Currency::Rub {
            Some(self.price_tier.price())
        } else {
            None
        }
    }

    /// Verify this offer against live marketplace data using a connector.
    pub async fn verify(
        &self,
        connector: &dyn crate::connector::MarketplaceConnector,
    ) -> Result<crate::services::OfferVerificationResult, crate::domain::ScraperError> {
        crate::services::verify_offer(connector, self).await
    }
}
