use serde::{Deserialize, Serialize};

use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier, ProductOffer, StockStatus};

/// Product listing tile extracted from a Taobao search page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaobaoSearchItem {
    pub item_id: String,
    pub title: String,
    pub price_cny: Option<f64>,
    pub original_price_cny: Option<f64>,
    pub shop_name: Option<String>,
    pub location: Option<String>,
    pub sales: Option<String>,
    pub sales_count: Option<u32>,
    pub url: String,
}

impl From<TaobaoSearchItem> for ProductOffer {
    fn from(item: TaobaoSearchItem) -> Self {
        let base_price = item.price_cny.map(|p| p.round() as u64).unwrap_or(0);
        let original_price = item.original_price_cny.map(|p| p.round() as u64);
        let stock = if base_price > 0 || item.price_cny.is_some() {
            StockStatus::InStock(1)
        } else {
            StockStatus::Unknown
        };

        ProductOffer {
            marketplace: Marketplace::Taobao,
            title: item.title,
            url: item.url,
            price_tier: PriceTier::Base(base_price),
            original_price,
            currency: Currency::Cny,
            rating: None,
            review_count: item.sales_count,
            stock,
            seller: item.shop_name,
            product_id: Some(item.item_id),
        }
    }
}

/// Detailed product card information extracted from an item.taobao.com page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaobaoCardDetails {
    pub item_id: String,
    pub title: String,
    pub price_cny: Option<f64>,
    pub original_price_cny: Option<f64>,
    pub shop_name: Option<String>,
    pub sales: Option<String>,
    pub sales_count: Option<u32>,
    pub location: Option<String>,
    pub description_images: usize,
    pub url: String,
}

impl From<TaobaoCardDetails> for ProductOffer {
    fn from(card: TaobaoCardDetails) -> Self {
        let base_price = card.price_cny.map(|p| p.round() as u64).unwrap_or(0);
        let original_price = card.original_price_cny.map(|p| p.round() as u64);
        let stock = if base_price > 0 || card.price_cny.is_some() {
            StockStatus::InStock(1)
        } else {
            StockStatus::Unknown
        };

        ProductOffer {
            marketplace: Marketplace::Taobao,
            title: card.title,
            url: card.url,
            price_tier: PriceTier::Base(base_price),
            original_price,
            currency: Currency::Cny,
            rating: None,
            review_count: card.sales_count,
            stock,
            seller: card.shop_name,
            product_id: Some(card.item_id),
        }
    }
}

/// Raw payload extracted from Taobao search DOM/JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TaobaoExtractedSearchPayload {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub anchors_total: Option<usize>,
    #[serde(default)]
    pub login_anchors: Option<usize>,
    #[serde(default)]
    pub body_snippet: Option<String>,
    #[serde(default)]
    pub items: Vec<TaobaoSearchItem>,
}

/// Raw payload extracted from Taobao card DOM/JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TaobaoExtractedCardPayload {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub price_cny: Option<f64>,
    #[serde(default)]
    pub original_price_cny: Option<f64>,
    #[serde(default)]
    pub shop_name: Option<String>,
    #[serde(default)]
    pub sales: Option<String>,
    #[serde(default)]
    pub description_images: Option<usize>,
    #[serde(default)]
    pub page_title: Option<String>,
    #[serde(default)]
    pub anchors_total: Option<usize>,
    #[serde(default)]
    pub login_anchors: Option<usize>,
    #[serde(default)]
    pub body_snippet: Option<String>,
    #[serde(default)]
    pub challenge_snippet: Option<String>,
}
