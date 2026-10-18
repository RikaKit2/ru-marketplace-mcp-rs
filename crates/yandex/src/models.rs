use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use ru_marketplace_core::domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, StockStatus,
};

/// Status returned by SSR parsing algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Ok,
    OkZone,
    OkLdjsonOnly,
    Empty,
    NoProductsFound,
    Captcha,
    EmptyProductShell,
}

impl ParseStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::OkZone => "ok_zone",
            Self::OkLdjsonOnly => "ok_ldjson_only",
            Self::Empty => "empty",
            Self::NoProductsFound => "no_products_found",
            Self::Captcha => "captcha",
            Self::EmptyProductShell => "empty_product_shell",
        }
    }
}

/// Yandex Market product as parsed from search SERP or widget collections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YandexProduct {
    pub product_id: Option<String>,
    pub sku_id: Option<String>,
    pub title: String,
    pub brand: Option<String>,
    pub seller: Option<String>,
    pub price_rub: Option<f64>,
    pub price_with_plus: Option<f64>,
    pub price_old_rub: Option<f64>,
    pub currency: String,
    pub rating: Option<f32>,
    pub rating_count: Option<u32>,
    pub in_stock: Option<bool>,
    pub is_express: bool,
    pub url: String,
    pub image: String,
    pub source: String,
}

impl From<YandexProduct> for ProductOffer {
    fn from(p: YandexProduct) -> Self {
        let base_price = p.price_rub.map(|v| v.round() as u64).unwrap_or(0);
        let plus_price = p.price_with_plus.map(|v| v.round() as u64);

        let price_tier = if let Some(plus) = plus_price
            && plus > 0
            && plus < base_price
        {
            PriceTier::MarketplaceCard {
                price: plus,
                card: CardType::YandexPay,
            }
        } else if base_price > 0 {
            PriceTier::Base(base_price)
        } else if let Some(plus) = plus_price
            && plus > 0
        {
            PriceTier::Base(plus)
        } else {
            PriceTier::Base(0)
        };

        let original_price = p.price_old_rub.map(|v| v.round() as u64);
        let stock = match p.in_stock {
            Some(true) => StockStatus::InStock(1),
            Some(false) => StockStatus::OutOfStock,
            None => StockStatus::InStock(1),
        };

        ProductOffer {
            marketplace: Marketplace::YandexMarket,
            title: p.title,
            url: p.url,
            price_tier,
            original_price,
            currency: Currency::Rub,
            rating: p.rating,
            review_count: p.rating_count,
            stock,
            seller: p.seller.filter(|s| !s.is_empty()),
            product_id: p.product_id.or(p.sku_id).filter(|id| !id.is_empty()),
        }
    }
}

/// Single customer review embedded in Yandex Market product SSR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YandexReview {
    pub author: String,
    pub rating: Option<u8>,
    pub date: String,
    pub pros: String,
    pub cons: String,
    pub comment: String,
    pub votes_up: i32,
    pub votes_down: i32,
    pub photos: Vec<String>,
}

/// Detailed product card attributes extracted from Yandex Market product page SSR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YandexCardDetails {
    pub status: ParseStatus,
    pub product_id: String,
    pub sku_id: Option<String>,
    pub title: String,
    pub brand: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub seller: Option<String>,
    pub price_rub: Option<f64>,
    pub price_with_plus: Option<f64>,
    pub price_before_discount_rub: Option<f64>,
    pub discount_percent: Option<u32>,
    pub currency: String,
    pub offers_count: Option<u32>,
    pub rating: Option<f32>,
    pub rating_count: Option<u32>,
    pub review_count: Option<u32>,
    pub rating_stars: HashMap<u8, u32>,
    pub reviews: Vec<YandexReview>,
}

impl From<YandexCardDetails> for ProductOffer {
    fn from(card: YandexCardDetails) -> Self {
        let base_price = card.price_rub.map(|v| v.round() as u64).unwrap_or(0);
        let plus_price = card.price_with_plus.map(|v| v.round() as u64);

        let price_tier = if let Some(plus) = plus_price
            && plus > 0
            && plus < base_price
        {
            PriceTier::MarketplaceCard {
                price: plus,
                card: CardType::YandexPay,
            }
        } else if base_price > 0 {
            PriceTier::Base(base_price)
        } else if let Some(plus) = plus_price
            && plus > 0
        {
            PriceTier::Base(plus)
        } else {
            PriceTier::Base(0)
        };

        let original_price = card.price_before_discount_rub.map(|v| v.round() as u64);
        let url = if !card.product_id.is_empty() {
            format!("https://market.yandex.ru/card/x/{}", card.product_id)
        } else {
            "https://market.yandex.ru".to_string()
        };

        ProductOffer {
            marketplace: Marketplace::YandexMarket,
            title: card.title,
            url,
            price_tier,
            original_price,
            currency: Currency::Rub,
            rating: card.rating,
            review_count: card.review_count.or(card.rating_count),
            stock: StockStatus::InStock(1),
            seller: card.seller.filter(|s| !s.is_empty()),
            product_id: Some(card.product_id).filter(|id| !id.is_empty()),
        }
    }
}

/// Structured search result container for Yandex Market queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YandexSearchResult {
    pub status: ParseStatus,
    pub query: String,
    pub total: Option<u64>,
    pub page: Option<u32>,
    pub page_count: Option<u32>,
    pub has_next_page: bool,
    pub items: Vec<YandexProduct>,
}
