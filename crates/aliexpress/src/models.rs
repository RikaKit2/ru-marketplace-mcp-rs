use serde::{Deserialize, Serialize};

use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, ProductOffer, StockStatus};

/// Product extracted from AliExpress catalog search or grid tiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliExpressProduct {
    /// 9-16 digit unique item ID.
    pub item_id: String,
    /// Product title extracted from the tile.
    pub title: String,
    /// Current customer price in rubles (None if absent, never 0).
    pub price_rub: Option<f64>,
    /// Base / strikethrough price in rubles.
    pub old_price_rub: Option<f64>,
    /// Coupon price in rubles if advertised with coupon badge.
    pub coupon_price_rub: Option<f64>,
    /// Discount percentage if present (e.g. 58 from "-58%").
    pub discount_percent: Option<u32>,
    /// Product rating (0.0 to 5.0).
    pub rating: Option<f32>,
    /// Number of orders / buyers ("N купили").
    pub orders_count: Option<u32>,
    /// SKU identifier if present in URL query parameters.
    pub sku_id: Option<String>,
    /// Canonical product URL.
    pub url: String,
    /// Product preview image URL.
    pub image_url: Option<String>,
    /// Seller / shop name if available.
    pub seller: Option<String>,
}

impl From<AliExpressProduct> for ProductOffer {
    fn from(p: AliExpressProduct) -> Self {
        let base_price = p.price_rub.map(|v| v.round() as u64).unwrap_or(0);
        let price_tier = if let Some(coupon) = p.coupon_price_rub
            && coupon > 0.0
            && coupon < p.price_rub.unwrap_or(f64::MAX)
        {
            PriceTier::CouponApplied {
                price: coupon.round() as u64,
                code: "aliexpress_coupon".to_string(),
            }
        } else {
            PriceTier::Base(base_price)
        };

        ProductOffer {
            marketplace: Marketplace::AliExpress,
            title: p.title,
            url: p.url,
            price_tier,
            original_price: p.old_price_rub.map(|v| v.round() as u64),
            currency: Currency::Rub,
            rating: p.rating,
            review_count: p.orders_count,
            stock: StockStatus::InStock(1),
            seller: p.seller.or_else(|| Some("AliExpress".to_string())),
            product_id: Some(p.item_id),
        }
    }
}

/// Detailed product card extracted from AliExpress product page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliExpressCard {
    /// 9-16 digit unique item ID.
    pub item_id: String,
    /// Product title (usually from h1 or meta).
    pub title: String,
    /// Current price in rubles.
    pub price_rub: Option<f64>,
    /// Base / strikethrough price in rubles.
    pub old_price_rub: Option<f64>,
    /// Product rating.
    pub rating: Option<f32>,
    /// Orders / buyer count.
    pub orders_count: Option<u32>,
    /// Canonical item URL.
    pub url: String,
    /// Product image URL.
    pub image_url: Option<String>,
    /// Meta description or summary.
    pub description: Option<String>,
    /// Seller / shop name.
    pub seller: Option<String>,
    /// Availability status.
    pub in_stock: bool,
}

impl From<AliExpressCard> for ProductOffer {
    fn from(c: AliExpressCard) -> Self {
        let base_price = c.price_rub.map(|v| v.round() as u64).unwrap_or(0);
        ProductOffer {
            marketplace: Marketplace::AliExpress,
            title: c.title,
            url: c.url,
            price_tier: PriceTier::Base(base_price),
            original_price: c.old_price_rub.map(|v| v.round() as u64),
            currency: Currency::Rub,
            rating: c.rating,
            review_count: c.orders_count,
            stock: if c.in_stock {
                StockStatus::InStock(1)
            } else {
                StockStatus::OutOfStock
            },
            seller: c.seller.or_else(|| Some("AliExpress".to_string())),
            product_id: Some(c.item_id),
        }
    }
}

/// Response payload for search operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliExpressSearchResponse {
    pub status: String,
    pub query: String,
    pub count: usize,
    pub items: Vec<AliExpressProduct>,
}

/// Response payload for card operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliExpressCardResponse {
    pub status: String,
    pub item_id: String,
    pub card: AliExpressCard,
}
