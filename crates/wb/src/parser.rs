use serde_json::Value;

use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
};

use std::collections::VecDeque;

use crate::models::{
    WbCategoriesResponse, WbCategoryNode, WbReviewItem, WbReviewsResponse, WbRootInfo, WbSellerInfo,
};

/// Best-effort decode of double-encoded UTF-8 strings.
/// If a string was encoded as UTF-8, then erroneously decoded as ISO-8859-1 / Latin-1,
/// all characters will have codepoints <= 0xFF.
/// Re-encoding them as bytes and parsing as UTF-8 recovers the original Cyrillic text.
pub fn decode_mojibake(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.chars().all(|c| (c as u32) <= 0xFF) {
        let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
        if let Ok(fixed) = std::str::from_utf8(&bytes) {
            let cyr_orig = s
                .chars()
                .filter(|c| ('\u{0400}'..='\u{04FF}').contains(c))
                .count();
            let cyr_fixed = fixed
                .chars()
                .filter(|c| ('\u{0400}'..='\u{04FF}').contains(c))
                .count();
            if cyr_fixed > cyr_orig {
                return fixed.to_string();
            }
        }
    }
    s.to_string()
}

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

        let title = decode_mojibake(&title);

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
            .map(decode_mojibake);

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

/// Parse full card metadata from Wildberries CDN basket info/ru/card.json.
pub fn parse_root_info_json(
    json_str: &str,
    nm_id: u64,
    host_used: Option<String>,
) -> Result<WbRootInfo, ScraperError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|e| ScraperError::Parse(format!("Failed to parse card.json: {e}")))?;

    let imt = root
        .get("imt_id")
        .or_else(|| root.get("imtId"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            root.get("data")
                .and_then(|d| d.get("imt_id").or_else(|| d.get("imtId")))
                .and_then(|v| v.as_u64())
        });

    let Some(imt_id) = imt else {
        return Err(ScraperError::Parse(format!(
            "imt_id missing in card.json for nm {nm_id}"
        )));
    };

    let subj_name = root
        .get("subj_name")
        .or_else(|| root.get("data").and_then(|d| d.get("subj_name")))
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .unwrap_or_default();

    let subj_root_name = root
        .get("subj_root_name")
        .or_else(|| root.get("data").and_then(|d| d.get("subj_root_name")))
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .unwrap_or_default();

    let vendor_code = root
        .get("vendor_code")
        .or_else(|| root.get("data").and_then(|d| d.get("vendor_code")))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let description = root
        .get("description")
        .or_else(|| root.get("data").and_then(|d| d.get("description")))
        .and_then(|v| v.as_str())
        .map(decode_mojibake);

    let options = root
        .get("options")
        .or_else(|| root.get("data").and_then(|d| d.get("options")))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(WbRootInfo {
        imt_id,
        subj_name,
        subj_root_name,
        vendor_code,
        description,
        options,
        host_used,
    })
}

/// Parse customer feedback and reviews JSON payload from feedbacks.wb.ru.
pub fn parse_reviews_json(
    json_str: &str,
    imt_id: u64,
    page: usize,
    host_used: Option<String>,
) -> Result<WbReviewsResponse, ScraperError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|e| ScraperError::Parse(format!("Failed to parse reviews JSON: {e}")))?;

    let feedback_count = root
        .get("feedbackCount")
        .or_else(|| root.get("feedbacksCount"))
        .and_then(|v| v.as_u64());

    let valuation = root.get("valuation").and_then(|v| {
        if let Some(s) = v.as_str() {
            Some(s.to_string())
        } else {
            v.as_f64().map(|n| format!("{n:.1}"))
        }
    });

    let mut items = Vec::new();
    if let Some(feedbacks_arr) = root.get("feedbacks").and_then(|v| v.as_array()) {
        for f in feedbacks_arr {
            let rating = f
                .get("productValuation")
                .or_else(|| f.get("valuation"))
                .or_else(|| f.get("stars"))
                .or_else(|| f.get("rating"))
                .and_then(|v| v.as_u64())
                .map(|r| r as u8);

            let text = f
                .get("text")
                .or_else(|| f.get("comment"))
                .and_then(|v| v.as_str())
                .map(decode_mojibake)
                .unwrap_or_default();

            let pros = f
                .get("pros")
                .and_then(|v| v.as_str())
                .map(decode_mojibake)
                .unwrap_or_default();

            let cons = f
                .get("cons")
                .and_then(|v| v.as_str())
                .map(decode_mojibake)
                .unwrap_or_default();

            let user = f
                .get("wbUserDetails")
                .and_then(|u| u.get("name"))
                .or_else(|| f.get("userName"))
                .or_else(|| f.get("name"))
                .and_then(|v| v.as_str())
                .map(decode_mojibake)
                .unwrap_or_default();

            let date = f
                .get("createdDate")
                .or_else(|| f.get("date"))
                .and_then(|v| v.as_str())
                .map(str::to_string);

            items.push(WbReviewItem {
                rating,
                text,
                pros,
                cons,
                user,
                date,
            });
        }
    }

    let page_size = 20;
    let page_num = page.max(1);
    let total_items = items.len();
    let total_pages = if total_items == 0 {
        1
    } else {
        total_items.div_ceil(page_size)
    };

    let skip = (page_num - 1) * page_size;
    let page_feedbacks = items.into_iter().skip(skip).take(page_size).collect();

    Ok(WbReviewsResponse {
        imt_id,
        feedback_count: feedback_count.or(Some(total_items as u64)),
        valuation,
        feedbacks: page_feedbacks,
        page: page_num,
        total_pages,
        host_used,
    })
}

/// Parse supplier registration metadata JSON payload from static CDN.
pub fn parse_seller_info_json(
    json_str: &str,
    supplier_id: u64,
    host_used: Option<String>,
) -> Result<WbSellerInfo, ScraperError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|e| ScraperError::Parse(format!("Failed to parse supplier JSON: {e}")))?;

    let supplier_name = root
        .get("supplierName")
        .or_else(|| root.get("name"))
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .unwrap_or_default()
        .trim()
        .to_string();

    if supplier_name.is_empty() {
        return Err(ScraperError::NotFound {
            query: format!("No seller record found for supplier_id {supplier_id}"),
        });
    }

    let full_name = root
        .get("supplierFullName")
        .or_else(|| root.get("fullName"))
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let inn = root
        .get("inn")
        .or_else(|| root.get("taxpayerCode"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.trim().to_string())
            } else {
                v.as_u64().map(|n| n.to_string())
            }
        })
        .filter(|s| !s.is_empty());

    let ogrn = root
        .get("ogrn")
        .or_else(|| root.get("ogrnip"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.trim().to_string())
            } else {
                v.as_u64().map(|n| n.to_string())
            }
        })
        .filter(|s| !s.is_empty());

    let legal_address = root
        .get("legalAddress")
        .or_else(|| root.get("address"))
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let trademark = root
        .get("trademark")
        .and_then(|v| v.as_str())
        .map(decode_mojibake)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let kpp = root
        .get("kpp")
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.trim().to_string())
            } else {
                v.as_u64().map(|n| n.to_string())
            }
        })
        .filter(|s| !s.is_empty());

    Ok(WbSellerInfo {
        supplier_id,
        supplier_name,
        full_name,
        inn,
        ogrn,
        legal_address,
        trademark,
        kpp,
        host_used,
    })
}

/// Maximum number of category nodes to traverse and return in one request.
pub const WB_MENU_MAX_NODES: usize = 400;

/// Breadth-first search for a category subtree by name, URL, or ID.
pub fn find_menu_subtree<'a>(nodes: &'a [Value], needle: &str) -> Option<&'a Value> {
    let needle_low = needle.trim().to_lowercase();
    if needle_low.is_empty() {
        return None;
    }
    let needle_trimmed = needle_low.trim_matches('/');
    let mut queue: VecDeque<&'a Value> = nodes.iter().collect();

    while let Some(node) = queue.pop_front() {
        let Some(obj) = node.as_object() else {
            continue;
        };
        let raw_name = obj.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        let name = decode_mojibake(raw_name).trim().to_lowercase();
        let url = obj
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let url_trimmed = url.trim_matches('/');
        let node_id = obj
            .get("id")
            .and_then(|v| {
                if let Some(n) = v.as_u64() {
                    Some(n.to_string())
                } else if let Some(n) = v.as_i64() {
                    Some(n.to_string())
                } else {
                    v.as_str().map(|s| s.trim().to_string())
                }
            })
            .unwrap_or_default();

        if needle_low == name
            || needle_low == url
            || needle_low == node_id
            || (!needle_trimmed.is_empty() && url_trimmed.ends_with(needle_trimmed))
            || url.ends_with(&format!("/{needle_low}"))
        {
            return Some(node);
        }

        if let Some(children) = obj.get("childs").and_then(|v| v.as_array()) {
            for child in children {
                queue.push_back(child);
            }
        }
    }

    None
}

/// Convert a raw menu JSON node into a bounded `WbCategoryNode`.
pub fn build_menu_node(
    raw: &Value,
    depth: usize,
    budget: &mut usize,
    max_depth: usize,
) -> Option<WbCategoryNode> {
    if *budget == 0 {
        return None;
    }
    let obj = raw.as_object()?;
    *budget -= 1;

    let children_slice = obj
        .get("childs")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let children_count = children_slice.len();

    let mut children = Vec::new();
    if depth < max_depth {
        for child in children_slice {
            if let Some(child_node) = build_menu_node(child, depth + 1, budget, max_depth) {
                children.push(child_node);
            } else {
                break;
            }
        }
    }

    let id = obj.get("id").and_then(|v| {
        v.as_u64()
            .or_else(|| {
                v.as_i64()
                    .and_then(|i| if i >= 0 { Some(i as u64) } else { None })
            })
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
    });
    let raw_name = obj.get("name").and_then(|v| v.as_str()).unwrap_or_default();
    let name = decode_mojibake(raw_name);
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let shard = obj
        .get("shard")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let query = obj
        .get("query")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Some(WbCategoryNode {
        id,
        name,
        url,
        shard,
        query,
        depth,
        children_count,
        childs: children,
    })
}

/// Parse raw WB main menu JSON array into `WbCategoriesResponse` with subtree & depth bounding.
pub fn parse_categories(
    data: &[Value],
    root: &str,
    max_depth: usize,
    host_used: Option<String>,
) -> Result<WbCategoriesResponse, ScraperError> {
    if data.is_empty() {
        return Err(ScraperError::Parse(
            "wb_categories: expected a non-empty menu array".to_string(),
        ));
    }

    let requested = root.trim();
    let (nodes, resolved) = if requested.is_empty()
        || requested.eq_ignore_ascii_case("top")
        || requested.eq_ignore_ascii_case("root")
        || requested.eq_ignore_ascii_case("all")
    {
        (data, "top".to_string())
    } else {
        let subtree = find_menu_subtree(data, requested).ok_or_else(|| {
            ScraperError::NotFound {
                query: format!(
                    "category '{requested}' not found in the WB menu — call get_categories(\"top\") to list sections"
                ),
            }
        })?;
        let children = subtree
            .get("childs")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]);
        let raw_name = subtree
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(requested);
        let resolved_name = decode_mojibake(raw_name);
        (children, resolved_name)
    };

    let max_depth = max_depth.max(1);
    let limit_depth = max_depth - 1;
    let mut budget = WB_MENU_MAX_NODES;
    let mut items = Vec::new();

    for raw in nodes {
        if let Some(node) = build_menu_node(raw, 0, &mut budget, limit_depth) {
            items.push(node);
        } else {
            break;
        }
    }

    let truncated = budget == 0;
    let total_returned = WB_MENU_MAX_NODES - budget;

    Ok(WbCategoriesResponse {
        root: resolved,
        max_depth,
        total_returned,
        truncated,
        items,
        host_used,
    })
}
