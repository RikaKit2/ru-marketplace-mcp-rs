use serde_json::Value;

use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
};

/// Parse raw Wildberries v9 search JSON payload into ProductOffers.
pub fn parse_products_json(
    json_str: &str,
    limit: usize,
) -> Result<Vec<ProductOffer>, ScraperError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|e| ScraperError::Parse(format!("Failed to parse WB JSON: {e}")))?;

    let products = root
        .get("data")
        .and_then(|d| d.get("products"))
        .and_then(|p| p.as_array())
        .or_else(|| root.get("products").and_then(|p| p.as_array()))
        .or_else(|| {
            root.get("payload")
                .and_then(|p| p.get("products"))
                .and_then(|p| p.as_array())
        });

    let Some(products_array) = products else {
        return Ok(Vec::new());
    };

    let mut offers = Vec::with_capacity(limit.min(products_array.len()));

    for p in products_array.iter().take(limit) {
        let Some(id) = p.get("id").and_then(|v| v.as_u64()) else {
            continue;
        };

        let title = p
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();

        if title.is_empty() {
            continue;
        }

        let (price_kopecks, original_kopecks) = extract_prices(p);

        let Some(kopecks) = price_kopecks else {
            continue;
        };

        let price_tier = PriceTier::from_kopecks(kopecks);
        let original_price = original_kopecks.map(|k| (k + 50) / 100);

        let rating = p
            .get("reviewRating")
            .or_else(|| p.get("rating"))
            .and_then(|v| v.as_f64())
            .map(|r| r as f32);

        let review_count = p
            .get("feedbacks")
            .or_else(|| p.get("reviewsCount"))
            .and_then(|v| v.as_u64())
            .map(|c| c as u32);

        let stock = match p.get("totalQuantity").and_then(|v| v.as_u64()) {
            Some(0) => StockStatus::OutOfStock,
            Some(qty) => StockStatus::InStock(qty as u32),
            None => StockStatus::Unknown,
        };

        let seller = p
            .get("supplier")
            .or_else(|| p.get("brand"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        offers.push(ProductOffer {
            marketplace: Marketplace::Wildberries,
            title,
            url: format!("https://www.wildberries.ru/catalog/{id}/detail.aspx"),
            price_tier,
            original_price,
            currency: Currency::Rub,
            rating,
            review_count,
            stock,
            seller,
            product_id: Some(id.to_string()),
        });
    }

    Ok(offers)
}

/// Extract (current_price_kopecks, original_price_kopecks) from product object.
pub fn extract_prices(p: &Value) -> (Option<u64>, Option<u64>) {
    // Check top-level price object (v18 / modern format)
    if let Some(price_obj) = p.get("price") {
        let current = price_obj
            .get("total")
            .or_else(|| price_obj.get("product"))
            .and_then(|v| v.as_u64())
            .filter(|&v| v > 0);
        let basic = price_obj
            .get("basic")
            .and_then(|v| v.as_u64())
            .filter(|&v| v > 0);
        if current.is_some() {
            return (current, basic.or(current));
        }
    }

    if let Some(sizes) = p.get("sizes").and_then(|v| v.as_array()) {
        for sz in sizes {
            if let Some(price_obj) = sz.get("price") {
                let current = price_obj
                    .get("product")
                    .or_else(|| price_obj.get("total"))
                    .and_then(|v| v.as_u64())
                    .filter(|&v| v > 0);

                let basic = price_obj
                    .get("basic")
                    .and_then(|v| v.as_u64())
                    .filter(|&v| v > 0);

                if current.is_some() {
                    return (current, basic.or(current));
                }
            }
        }
    }

    let sale = p
        .get("salePriceU")
        .and_then(|v| v.as_u64())
        .filter(|&v| v > 0);
    let base = p.get("priceU").and_then(|v| v.as_u64()).filter(|&v| v > 0);

    if sale.is_some() || base.is_some() {
        let current = sale.or(base);
        let original = base.or(sale);
        return (current, original);
    }

    (None, None)
}
