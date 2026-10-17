use serde_json::Value;

use ru_marketplace_core::domain::{
    CardType, Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
    WafProtection,
};

/// Parse price strings like "1 499 ₽", "12 490 ₽", "1499" into integer rubles.
pub fn parse_price_str(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok().filter(|&p| p > 0)
}

/// Parse an Ozon Composer API JSON response into product offers.
pub fn parse_composer_json(
    json_str: &str,
    limit: usize,
) -> Result<Vec<ProductOffer>, ScraperError> {
    let trimmed = json_str.trim();
    let root: Value = if trimmed.starts_with('<') {
        if let Some(widget_state) = extract_between(trimmed, "\"widgetStates\":", ",\"layout\":") {
            let wrapped = format!("{{\"widgetStates\":{widget_state}}}");
            serde_json::from_str(&wrapped).map_err(|e| {
                ScraperError::Parse(format!("Failed to parse Ozon widgetStates from HTML: {e}"))
            })?
        } else if let Some(state_json) =
            extract_between(trimmed, "window.__INITIAL_STATE__ = ", ";</script>")
        {
            serde_json::from_str(state_json).map_err(|e| {
                ScraperError::Parse(format!("Failed to parse Ozon __INITIAL_STATE__: {e}"))
            })?
        } else {
            serde_json::from_str(trimmed)
                .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon JSON: {e}")))?
        }
    } else {
        serde_json::from_str(trimmed)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon JSON: {e}")))?
    };
    if let Some(inc) = root
        .get("incidentId")
        .or_else(|| root.get("incident_id"))
        .and_then(|v| v.as_str())
    {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Variti,
            details: format!("Ozon security incident: {inc}"),
        });
    }

    let widgets = match root.get("widgetStates") {
        Some(Value::Object(map)) => map,
        _ => {
            if let Some(err) = root
                .get("error")
                .or_else(|| root.get("message"))
                .and_then(|v| v.as_str())
            {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Ozon error response: {err}"),
                });
            }
            return Ok(Vec::new());
        }
    };

    let mut offers = Vec::new();

    for (key, val) in widgets {
        let is_search_widget = key.starts_with("tileGridDesktop-")
            || key.starts_with("searchResultsV2-")
            || key.starts_with("tileGrid-")
            || key.starts_with("catalog-")
            || key.starts_with("megaPaginator-");
        if !is_search_widget {
            continue;
        }
        let widget_obj: Value = if let Some(s) = val.as_str() {
            serde_json::from_str(s).unwrap_or(Value::Null)
        } else {
            val.clone()
        };

        let Some(items) = widget_obj.get("items").and_then(|v| v.as_array()) else {
            continue;
        };

        for item in items {
            if offers.len() >= limit {
                break;
            }

            if let Some(offer) = parse_search_tile(item) {
                offers.push(offer);
            }
        }

        if offers.len() >= limit {
            break;
        }
    }

    Ok(offers)
}

/// Parse a single search tile item from Ozon mainState atoms.
fn parse_search_tile(item: &Value) -> Option<ProductOffer> {
    let sku = item
        .get("sku")
        .or_else(|| item.get("id"))
        .map(|v| match v {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty());

    let raw_link = item
        .get("action")
        .and_then(|a| a.get("link"))
        .and_then(|l| l.as_str())
        .unwrap_or_default();

    let url = if raw_link.starts_with("http") {
        raw_link.to_string()
    } else if !raw_link.is_empty() {
        format!(
            "https://www.ozon.ru{}",
            raw_link.split('?').next().unwrap_or(raw_link)
        )
    } else if let Some(id) = &sku {
        format!("https://www.ozon.ru/product/{id}/")
    } else {
        return None;
    };

    let main_state = item.get("mainState").and_then(|v| v.as_array())?;

    let mut title = String::new();
    let mut price: u64 = 0;
    let mut price_original: Option<u64> = None;
    let mut has_card_price = false;
    let mut rating: Option<f32> = None;
    let mut review_count: Option<u32> = None;
    let mut stock = StockStatus::Unknown;

    for atom in main_state {
        let atom_type = atom
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        match atom_type {
            "textAtom" => {
                let atom_obj = atom.get("textAtom").unwrap_or(atom);
                let text = atom_obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if title.is_empty()
                    && text.len() > 10
                    && !text.contains('₽')
                    && !text.contains('%')
                    && !text.contains("балл")
                    && !text.contains("доставк")
                    && !text.starts_with('+')
                {
                    title = text.to_string();
                }

                if text.contains("отзыв") || text.contains("оцен") {
                    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(count) = digits.parse::<u32>() {
                        review_count = Some(count);
                    }
                }
            }
            "price" | "priceV2" => {
                let atom_obj = atom
                    .get("price")
                    .or_else(|| atom.get("priceV2"))
                    .unwrap_or(atom);

                if let Some(p_str) = atom_obj.get("price").and_then(|v| v.as_str())
                    && let Some(p) = parse_price_str(p_str)
                {
                    price = p;
                }

                if let Some(op_str) = atom_obj.get("originalPrice").and_then(|v| v.as_str()) {
                    price_original = parse_price_str(op_str);
                }

                if let Some(theme) = atom_obj
                    .get("theme")
                    .and_then(|t| t.get("textColor"))
                    .and_then(|c| c.as_str())
                    && (theme.contains("ozon") || theme.contains("card") || theme.contains("green"))
                {
                    has_card_price = true;
                }

                if let Some(card_label) = atom_obj.get("priceColor").and_then(|c| c.as_str())
                    && (card_label.contains("green") || card_label.contains("ozon"))
                {
                    has_card_price = true;
                }
            }
            "labelList" => {
                let atom_obj = atom.get("labelList").unwrap_or(atom);
                if let Some(items) = atom_obj.get("items").and_then(|i| i.as_array()) {
                    for label in items {
                        let text = label
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if (text.starts_with("4.")
                            || text.starts_with("5.")
                            || text.starts_with("3."))
                            && text.len() <= 4
                            && let Ok(r) = text.parse::<f32>()
                        {
                            rating = Some(r);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if price == 0 {
        return None;
    }

    if title.is_empty() {
        if let Some(id) = &sku {
            title = format!("Ozon Product {id}");
        } else {
            return None;
        }
    }

    let price_tier = if has_card_price {
        PriceTier::MarketplaceCard {
            price,
            card: CardType::OzonCard,
        }
    } else {
        PriceTier::Base(price)
    };

    if stock == StockStatus::Unknown {
        stock = StockStatus::InStock(1);
    }

    Some(ProductOffer {
        marketplace: Marketplace::Ozon,
        title,
        url,
        price_tier,
        original_price: price_original,
        currency: Currency::Rub,
        rating,
        review_count,
        stock,
        seller: None,
        product_id: sku,
    })
}

/// Parse an Ozon product card Composer API JSON response.
pub fn parse_card_json(json_str: &str, item_id: &str) -> Result<ProductOffer, ScraperError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon card JSON: {e}")))?;

    if let Some(inc) = root
        .get("incidentId")
        .or_else(|| root.get("incident_id"))
        .and_then(|v| v.as_str())
    {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Variti,
            details: format!("Ozon security incident: {inc}"),
        });
    }

    let widgets = match root.get("widgetStates") {
        Some(Value::Object(map)) => map,
        _ => {
            if let Some(err) = root
                .get("error")
                .or_else(|| root.get("message"))
                .and_then(|v| v.as_str())
            {
                return Err(ScraperError::WafBlocked {
                    waf: WafProtection::Variti,
                    details: format!("Ozon error response: {err}"),
                });
            }
            return Err(ScraperError::NotFound {
                query: item_id.to_string(),
            });
        }
    };

    let mut title = String::new();
    let mut base_price = 0u64;
    let mut card_price = 0u64;
    let mut original_price = None;
    let mut is_available = true;
    let mut rating = None;
    let mut review_count = None;
    let mut seller = None;

    for (key, val) in widgets {
        let widget_obj: Value = if let Some(s) = val.as_str() {
            serde_json::from_str(s).unwrap_or(Value::Null)
        } else {
            val.clone()
        };

        if key.starts_with("webProductHeading") {
            if let Some(t) = widget_obj.get("title").and_then(|v| v.as_str()) {
                title = t.to_string();
            }
        } else if key.starts_with("webPrice-") {
            if let Some(avail) = widget_obj.get("isAvailable").and_then(|v| v.as_bool()) {
                is_available = avail;
            }
            if let Some(p_str) = widget_obj.get("price").and_then(|v| v.as_str()) {
                base_price = parse_price_str(p_str).unwrap_or(0);
            }
            if let Some(cp_str) = widget_obj.get("cardPrice").and_then(|v| v.as_str()) {
                card_price = parse_price_str(cp_str).unwrap_or(0);
            }
            if let Some(orig_str) = widget_obj.get("originalPrice").and_then(|v| v.as_str()) {
                original_price = parse_price_str(orig_str);
            }
        } else if key.starts_with("webReviewProductScore") {
            if let Some(score) = widget_obj.get("totalScore").and_then(|v| v.as_f64()) {
                rating = Some(score as f32);
            }
            if let Some(count) = widget_obj.get("reviewsCount").and_then(|v| v.as_u64()) {
                review_count = Some(count as u32);
            }
        } else if key.starts_with("webBrand")
            && seller.is_none()
            && let Some(brand_name) = widget_obj
                .get("content")
                .and_then(|c| c.get("title"))
                .and_then(|t| t.get("text"))
                .and_then(|arr| arr.get(0))
                .and_then(|item| item.get("content"))
                .and_then(|v| v.as_str())
        {
            seller = Some(brand_name.to_string());
        } else if key.starts_with("webCurrentSeller")
            && let Some(s_name) = widget_obj
                .get("name")
                .or_else(|| widget_obj.get("title"))
                .and_then(|v| v.as_str())
        {
            seller = Some(s_name.to_string());
        }
    }

    if title.is_empty() {
        return Err(ScraperError::NotFound {
            query: item_id.to_string(),
        });
    }

    let price_tier = if card_price > 0 && card_price < base_price {
        PriceTier::MarketplaceCard {
            price: card_price,
            card: CardType::OzonCard,
        }
    } else if base_price > 0 {
        PriceTier::Base(base_price)
    } else if card_price > 0 {
        PriceTier::Base(card_price)
    } else {
        PriceTier::Base(0)
    };

    let stock = if is_available {
        StockStatus::InStock(1)
    } else {
        StockStatus::OutOfStock
    };

    Ok(ProductOffer {
        marketplace: Marketplace::Ozon,
        title,
        url: format!("https://www.ozon.ru/product/{item_id}/"),
        price_tier,
        original_price,
        currency: Currency::Rub,
        rating,
        review_count,
        stock,
        seller,
        product_id: Some(item_id.to_string()),
    })
}

fn extract_between<'a>(haystack: &'a str, start_tag: &str, end_tag: &str) -> Option<&'a str> {
    let start_idx = haystack.find(start_tag)? + start_tag.len();
    let rest = &haystack[start_idx..];
    if end_tag.is_empty() {
        return Some(rest);
    }
    let end_idx = rest.find(end_tag)?;
    Some(&rest[..end_idx])
}
