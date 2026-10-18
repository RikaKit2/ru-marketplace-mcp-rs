use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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
            || key.starts_with("searchResultsV3-")
            || key.starts_with("searchMergedDesktop-")
            || key.starts_with("tileGrid-")
            || key.starts_with("catalog-")
            || key.starts_with("megaPaginator-")
            || key.starts_with("shelf-");
        if !is_search_widget {
            continue;
        }
        let widget_obj: Value = if let Some(s) = val.as_str() {
            serde_json::from_str(s).unwrap_or(Value::Null)
        } else {
            val.clone()
        };

        // Extract items array from multiple possible keys
        let items: Vec<&Value> =
            if let Some(arr) = widget_obj.get("items").and_then(|v| v.as_array()) {
                arr.iter().collect()
            } else if let Some(arr) = widget_obj.get("products").and_then(|v| v.as_array()) {
                arr.iter().collect()
            } else if let Some(arr) = widget_obj.get("elements").and_then(|v| v.as_array()) {
                arr.iter().collect()
            } else if let Some(arr) = widget_obj
                .get("cellTrackingInfo")
                .and_then(|v| v.as_array())
            {
                arr.iter().collect()
            } else {
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

    let main_state = item.get("mainState").and_then(|v| v.as_array());

    let mut title = String::new();
    let mut price: u64 = 0;
    let mut price_original: Option<u64> = None;
    let mut has_card_price = false;
    let mut rating: Option<f32> = None;
    let mut review_count: Option<u32> = None;
    let mut stock = StockStatus::Unknown;

    if let Some(main_state) = main_state {
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
                        .unwrap_or_default()
                        .trim();

                    if title.is_empty()
                        && !text.is_empty()
                        && !text.contains('₽')
                        && !text.contains('%')
                        && !text.contains("балл")
                        && !text.contains("доставк")
                        && !text.contains("корзин")
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
                "price" | "priceV2" | "priceV3" | "splitPrice" => {
                    let atom_obj = atom
                        .get("price")
                        .or_else(|| atom.get("priceV2"))
                        .or_else(|| atom.get("priceV3"))
                        .or_else(|| atom.get("splitPrice"))
                        .unwrap_or(atom);

                    if let Some(price_items) = atom_obj.get("price").and_then(|v| v.as_array()) {
                        for pi in price_items {
                            let text = pi
                                .get("text")
                                .or_else(|| pi.get("price"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let theme = pi
                                .get("theme")
                                .or_else(|| pi.get("textColor"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();

                            if let Some(p) = parse_price_str(text) {
                                if price == 0 {
                                    price = p;
                                    if theme.contains("card")
                                        || theme.contains("green")
                                        || theme.contains("ozon_card")
                                        || theme.contains("ozon")
                                    {
                                        has_card_price = true;
                                    }
                                } else if price_original.is_none() && p > price {
                                    price_original = Some(p);
                                }
                            }
                        }
                    } else if let Some(p_str) = atom_obj.get("price").and_then(|v| v.as_str())
                        && let Some(p) = parse_price_str(p_str)
                    {
                        price = p;
                    }

                    if let Some(op_str) = atom_obj.get("originalPrice").and_then(|v| v.as_str())
                        && let Some(op) = parse_price_str(op_str)
                    {
                        price_original = Some(op);
                    }

                    if let Some(theme) = atom_obj
                        .get("theme")
                        .and_then(|t| t.get("textColor").or(Some(t)))
                        .and_then(|c| c.as_str())
                        && (theme.contains("ozon")
                            || theme.contains("card")
                            || theme.contains("green"))
                    {
                        has_card_price = true;
                    }

                    if let Some(card_label) = atom_obj.get("priceColor").and_then(|c| c.as_str())
                        && (card_label.contains("green")
                            || card_label.contains("ozon")
                            || card_label.contains("card"))
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
    }

    // Fallback if mainState is absent or yields empty title/price: check top-level item attributes
    if title.is_empty()
        && let Some(t) = item
            .get("title")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    {
        title = t.to_string();
    }

    if price == 0 {
        let top_price = item
            .get("price")
            .or_else(|| item.get("priceModel"))
            .or_else(|| item.get("cellTrackingInfo"));

        if let Some(tp) = top_price {
            if let Some(p_str) = tp.as_str() {
                if let Some(p) = parse_price_str(p_str) {
                    price = p;
                }
            } else if let Some(p_num) = tp.as_u64() {
                price = p_num;
            } else if let Some(p_obj) = tp.as_object() {
                if let Some(p) = p_obj
                    .get("price")
                    .or_else(|| p_obj.get("finalPrice"))
                    .or_else(|| p_obj.get("value"))
                {
                    if let Some(p_str) = p.as_str() {
                        if let Some(p_val) = parse_price_str(p_str) {
                            price = p_val;
                        }
                    } else if let Some(p_num) = p.as_u64() {
                        price = p_num;
                    }
                }
                if let Some(theme) = p_obj.get("theme").and_then(|v| v.as_str())
                    && (theme.contains("card") || theme.contains("green") || theme.contains("ozon"))
                {
                    has_card_price = true;
                }
            }
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

/// An individual review item extracted from Ozon's webReviewList widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OzonReviewItem {
    /// Review UUID or ID.
    pub id: Option<String>,
    /// Review author name.
    pub author: Option<String>,
    /// Date of publication (ISO-8601 or raw date string).
    pub date: Option<String>,
    /// Review star score from 1 to 5.
    pub score: Option<u8>,
    /// Positive aspects / pros ("достоинства").
    pub positive: Option<String>,
    /// Negative aspects / cons ("недостатки").
    pub negative: Option<String>,
    /// Main comment / review body text ("комментарий").
    pub comment: Option<String>,
    /// Text alias for comment.
    pub text: Option<String>,
    /// Number of helpful / like votes.
    pub useful: Option<i32>,
    /// Number of unhelpful / dislike votes.
    pub unuseful: Option<i32>,
    /// Likes alias for useful.
    pub likes: Option<i32>,
    /// Dislikes alias for unuseful.
    pub dislikes: Option<i32>,
    /// Number of attached photos.
    pub photos: usize,
    /// Number of attached photos (alias).
    pub photos_count: usize,
    /// List of photo URLs if available.
    pub photo_urls: Vec<String>,
    /// Number of attached videos.
    pub videos: usize,
    /// Number of attached videos (alias).
    pub videos_count: usize,
    /// List of video URLs if available.
    pub video_urls: Vec<String>,
    /// SKU of the specific variant/product this review was written for in the card family pool.
    pub item_id: Option<u64>,
}

impl OzonReviewItem {
    pub fn rating(&self) -> Option<u8> {
        self.score
    }
    pub fn pros(&self) -> Option<&str> {
        self.positive.as_deref()
    }
    pub fn cons(&self) -> Option<&str> {
        self.negative.as_deref()
    }
    pub fn review_text(&self) -> Option<&str> {
        self.comment.as_deref().or(self.text.as_deref())
    }
}

/// Aggregate review score and star distribution from Ozon's webReviewScore widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OzonReviewScore {
    /// Average aggregate rating score (e.g. 4.8).
    pub rating_score: Option<f32>,
    /// Average aggregate rating score (alias).
    pub average_rating: Option<f32>,
    /// Total count of reviews.
    pub reviews_count: Option<u64>,
    /// Total count of reviews (alias).
    pub total_count: Option<u64>,
    /// Star distribution: star rating (1..5) -> count.
    pub distribution: HashMap<u8, u64>,
}

impl OzonReviewScore {
    pub fn new(
        rating_score: Option<f32>,
        reviews_count: Option<u64>,
        distribution: HashMap<u8, u64>,
    ) -> Self {
        Self {
            rating_score,
            average_rating: rating_score,
            reviews_count,
            total_count: reviews_count,
            distribution,
        }
    }
}

/// Product aspect evaluated by buyers (e.g. quality, delivery, match description) from webAspects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OzonAspectItem {
    /// Aspect name (e.g. "Качество", "Доставка", "Соответствие описанию").
    pub name: String,
    /// Average rating score for this aspect if present.
    pub score: Option<f32>,
    /// Number of positive votes/mentions.
    pub positive_count: Option<u64>,
    /// Number of negative votes/mentions.
    pub negative_count: Option<u64>,
    /// Total mentions/reviews evaluating this aspect.
    pub total_count: Option<u64>,
    /// Sentiment tag (e.g. "positive", "neutral", "negative").
    pub sentiment: Option<String>,
}

/// Full response for Ozon reviews Composer API endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OzonReviewsResponse {
    /// Operation status ("success").
    pub status: String,
    /// Requested SKU string.
    pub sku: String,
    /// Requested page number.
    pub page: usize,
    /// Requested sort option.
    pub sort: String,
    /// Aggregate score structure from webReviewScore.
    pub score: Option<OzonReviewScore>,
    /// Convenient top-level average rating score.
    pub rating_score: Option<f32>,
    /// Convenient top-level total reviews count.
    pub reviews_count: Option<u64>,
    /// Convenient top-level star distribution (1..5 -> count).
    pub distribution: HashMap<u8, u64>,
    /// List of reviews from webReviewList.
    pub reviews: Vec<OzonReviewItem>,
    /// List of product aspects from webAspects.
    pub aspects: Vec<OzonAspectItem>,
    /// Number of reviews returned in this page.
    pub returned: usize,
    /// Numeric SKU if requested sku was valid digits.
    pub requested_item_id: Option<u64>,
    /// Number of reviews specifically belonging to requested_item_id.
    pub own_reviews: usize,
    /// Pool variants mapping SKU -> product name.
    pub pool_variants: HashMap<String, String>,
    /// Whether there are more pages available.
    pub has_next: bool,
}

/// Extract an Ozon SKU from a plain numeric string, URL, or product path.
pub fn extract_sku(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    let path = if let Some(idx) = trimmed.find("/product/") {
        &trimmed[idx..]
    } else {
        trimmed
    };

    if let Some(rest) = path.strip_prefix("/product/") {
        let clean = rest.split('?').next().unwrap_or(rest);
        let clean = clean.split('#').next().unwrap_or(clean);
        let segments: Vec<&str> = clean
            .split('/')
            .filter(|s| !s.is_empty() && *s != "reviews")
            .collect();
        if let Some(first_segment) = segments.first() {
            if first_segment.chars().all(|c| c.is_ascii_digit()) {
                return Some(first_segment.to_string());
            }
            if let Some(last_dash) = first_segment.rfind('-') {
                let candidate = &first_segment[last_dash + 1..];
                if !candidate.is_empty() && candidate.chars().all(|c| c.is_ascii_digit()) {
                    return Some(candidate.to_string());
                }
            }
        }
    }

    if let Some(digits) = trimmed
        .rsplit(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty() && s.len() >= 4)
    {
        return Some(digits.to_string());
    }

    None
}

/// Parse an Ozon Composer API reviews response into an `OzonReviewsResponse`.
pub fn parse_reviews_json(
    json_str: &str,
    requested_sku: &str,
    page: usize,
    sort: &str,
) -> Result<OzonReviewsResponse, ScraperError> {
    let trimmed = json_str.trim();
    let root: Value = if trimmed.starts_with('<') {
        if trimmed.contains("challenge-platform") || trimmed.contains("cf-turnstile") {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::CloudflareTurnstile,
                details: "Cloudflare Turnstile challenge detected in HTML".to_string(),
            });
        }
        return Err(ScraperError::Parse(
            "Received HTML instead of JSON Composer response".to_string(),
        ));
    } else {
        serde_json::from_str(trimmed)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse Ozon reviews JSON: {e}")))?
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
            return Err(ScraperError::Parse(
                "Expected object payload with widgetStates object".to_string(),
            ));
        }
    };

    let mut score_opt: Option<OzonReviewScore> = None;
    let mut reviews = Vec::new();
    let mut aspects = Vec::new();
    let mut pool_variants = HashMap::new();
    let mut has_next = false;
    let mut total_from_paging = None;

    for (key, val) in widgets {
        let widget_obj: Value = if let Some(s) = val.as_str() {
            serde_json::from_str(s).unwrap_or(Value::Null)
        } else {
            val.clone()
        };

        // 1. webReviewScore / webReviewProductScore
        if key.starts_with("webReviewProductScore")
            || key.starts_with("webReviewScore")
            || key.starts_with("webScore")
        {
            let rating_score = widget_obj
                .get("totalScore")
                .or_else(|| widget_obj.get("score"))
                .or_else(|| widget_obj.get("rating"))
                .and_then(|v| v.as_f64())
                .map(|f| f as f32);

            let reviews_count = widget_obj
                .get("reviewsCount")
                .or_else(|| widget_obj.get("count"))
                .or_else(|| widget_obj.get("total"))
                .and_then(|v| v.as_u64());

            let mut distribution = HashMap::new();
            let score_rows = widget_obj
                .get("score")
                .or_else(|| widget_obj.get("distribution"))
                .or_else(|| widget_obj.get("ratingDistribution"))
                .and_then(|v| v.as_array());

            if let Some(rows) = score_rows {
                for row in rows {
                    let count = row
                        .get("value")
                        .or_else(|| row.get("count"))
                        .and_then(|v| v.as_u64());

                    let star = if let Some(title) = row.get("title").and_then(|v| v.as_str()) {
                        title
                            .chars()
                            .find(|c| c.is_ascii_digit())
                            .and_then(|c| c.to_digit(10))
                            .map(|d| d as u8)
                    } else {
                        row.get("stars")
                            .or_else(|| row.get("score"))
                            .and_then(|v| v.as_u64())
                            .map(|s| s as u8)
                    };

                    if let (Some(s), Some(c)) = (star, count) {
                        distribution.insert(s, c);
                    }
                }
            }

            score_opt = Some(OzonReviewScore::new(
                rating_score,
                reviews_count,
                distribution,
            ));
        }
        // 2. webReviewList / webListReviews
        else if key.starts_with("webListReviews")
            || key.starts_with("webReviewList")
            || key.starts_with("webReviews")
        {
            if let Some(products_map) = widget_obj.get("products").and_then(|v| v.as_object()) {
                for (v_sku, info) in products_map {
                    if let Some(name) = info.get("name").and_then(|v| v.as_str()) {
                        pool_variants
                            .entry(v_sku.clone())
                            .or_insert_with(|| name.to_string());
                    }
                }
            }

            if let Some(paging) = widget_obj.get("paging").and_then(|v| v.as_object()) {
                if let Some(total) = paging.get("total").and_then(|v| v.as_u64()) {
                    total_from_paging = Some(total);
                }
                if let Some(nb) = paging.get("nextButton")
                    && let Some(s) = nb.as_str()
                    && !s.is_empty()
                {
                    has_next = true;
                }
            }

            if let Some(rev_arr) = widget_obj.get("reviews").and_then(|v| v.as_array()) {
                for r in rev_arr {
                    if let Some(item) = parse_review_item(r) {
                        reviews.push(item);
                    }
                }
            }
        }
        // 3. webAspects
        else if key.starts_with("webAspects")
            || key.starts_with("aspects")
            || key.starts_with("webReviewAspects")
        {
            let aspect_arr = if let Some(arr) = widget_obj.as_array() {
                Some(arr)
            } else {
                widget_obj
                    .get("aspects")
                    .or_else(|| widget_obj.get("items"))
                    .or_else(|| widget_obj.get("aspectList"))
                    .or_else(|| widget_obj.get("list"))
                    .and_then(|v| v.as_array())
            };

            if let Some(arr) = aspect_arr {
                for a in arr {
                    if let Some(aspect) = parse_aspect_item(a) {
                        aspects.push(aspect);
                    }
                }
            }
        }
    }

    let req_item_id = requested_sku.parse::<u64>().ok();
    let own_reviews = if let Some(target_id) = req_item_id {
        reviews
            .iter()
            .filter(|r| r.item_id == Some(target_id))
            .count()
    } else {
        reviews.len()
    };

    let rating_score = score_opt.as_ref().and_then(|s| s.rating_score);
    let reviews_count = score_opt
        .as_ref()
        .and_then(|s| s.reviews_count)
        .or(total_from_paging);
    let distribution = score_opt
        .as_ref()
        .map(|s| s.distribution.clone())
        .unwrap_or_default();

    Ok(OzonReviewsResponse {
        status: "success".to_string(),
        sku: requested_sku.to_string(),
        page,
        sort: sort.to_string(),
        score: score_opt,
        rating_score,
        reviews_count,
        distribution,
        reviews,
        aspects,
        returned: 0, // updated right below
        requested_item_id: req_item_id,
        own_reviews,
        pool_variants,
        has_next,
    })
    .map(|mut r| {
        r.returned = r.reviews.len();
        r
    })
}

fn extract_text_field(v: Option<&Value>) -> Option<String> {
    let val = v?;
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    } else if let Some(obj) = val.as_object() {
        if let Some(s) = obj
            .get("text")
            .or_else(|| obj.get("value"))
            .or_else(|| obj.get("content"))
            .and_then(|t| t.as_str())
        {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    } else if let Some(arr) = val.as_array() {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    let t = s.trim();
                    if !t.is_empty() {
                        Some(t.to_string())
                    } else {
                        None
                    }
                } else if let Some(obj) = item.as_object() {
                    obj.get("text")
                        .or_else(|| obj.get("content"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|t| !t.is_empty())
                } else {
                    None
                }
            })
            .collect();
        if !parts.is_empty() {
            return Some(parts.join(" "));
        }
    }
    None
}

fn format_timestamp(mut secs: i64) -> String {
    if secs > 10_000_000_000 {
        secs /= 1000;
    }
    if secs < 0 {
        return secs.to_string();
    }
    let z = secs / 86400 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let time = secs % 86400;
    let hour = time / 3600;
    let min = (time % 3600) / 60;
    let sec = time % 60;

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn parse_review_item(r: &Value) -> Option<OzonReviewItem> {
    if !r.is_object() {
        return None;
    }

    let id = r
        .get("uuid")
        .or_else(|| r.get("id"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty());

    let item_id = r
        .get("itemId")
        .or_else(|| r.get("item_id"))
        .or_else(|| r.get("sku"))
        .and_then(|v| {
            if let Some(n) = v.as_u64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.parse::<u64>().ok()
            } else {
                None
            }
        });

    let author = r.get("author").and_then(|a| {
        if let Some(s) = a.as_str() {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        } else if let Some(obj) = a.as_object() {
            let fn_str = obj
                .get("firstName")
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let ln_str = obj
                .get("lastName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let full = match (!fn_str.is_empty(), !ln_str.is_empty()) {
                (true, true) => format!("{fn_str} {ln_str}"),
                (true, false) => fn_str.to_string(),
                (false, true) => ln_str.to_string(),
                (false, false) => return None,
            };
            return Some(full);
        }
        None
    });

    let date = r
        .get("publishedAt")
        .or_else(|| r.get("createdAt"))
        .or_else(|| r.get("date"))
        .or_else(|| r.get("published_at"))
        .and_then(|d| match d {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => {
                if let Some(ts) = n.as_i64() {
                    Some(format_timestamp(ts))
                } else {
                    n.as_f64().map(|ts_f| format_timestamp(ts_f as i64))
                }
            }
            _ => None,
        });

    let content = r.get("content");
    let content_obj = content.and_then(|c| c.as_object());

    let score = content_obj
        .and_then(|c| c.get("score").or_else(|| c.get("rating")))
        .or_else(|| r.get("score").or_else(|| r.get("rating")))
        .and_then(|v| v.as_u64())
        .map(|s| s as u8);

    let comment =
        extract_text_field(content_obj.and_then(|c| c.get("comment").or_else(|| c.get("text"))))
            .or_else(|| extract_text_field(r.get("comment").or_else(|| r.get("text"))));
    let positive =
        extract_text_field(content_obj.and_then(|c| c.get("positive").or_else(|| c.get("pros"))))
            .or_else(|| extract_text_field(r.get("positive").or_else(|| r.get("pros"))));
    let negative =
        extract_text_field(content_obj.and_then(|c| c.get("negative").or_else(|| c.get("cons"))))
            .or_else(|| extract_text_field(r.get("negative").or_else(|| r.get("cons"))));

    let usefulness = r.get("usefulness").and_then(|u| u.as_object());
    let useful = usefulness
        .and_then(|u| u.get("useful").or_else(|| u.get("likes")))
        .or_else(|| r.get("useful").or_else(|| r.get("likes")))
        .and_then(|v| v.as_i64())
        .map(|n| n as i32);

    let unuseful = usefulness
        .and_then(|u| {
            u.get("unuseful")
                .or_else(|| u.get("dislike"))
                .or_else(|| u.get("dislikes"))
        })
        .or_else(|| r.get("unuseful").or_else(|| r.get("dislikes")))
        .and_then(|v| v.as_i64())
        .map(|n| n as i32);

    let mut photo_urls = Vec::new();
    let raw_photos = content_obj
        .and_then(|c| c.get("photos"))
        .or_else(|| r.get("photos"));
    if let Some(arr) = raw_photos.and_then(|p| p.as_array()) {
        for p in arr {
            if let Some(url) = p.as_str() {
                photo_urls.push(url.to_string());
            } else if let Some(url) = p
                .get("url")
                .or_else(|| p.get("src"))
                .or_else(|| p.get("link"))
                .and_then(|v| v.as_str())
            {
                photo_urls.push(url.to_string());
            }
        }
    }
    let photos_count = if photo_urls.is_empty() {
        raw_photos
            .and_then(|p| p.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    } else {
        photo_urls.len()
    };

    let mut video_urls = Vec::new();
    let raw_videos = content_obj
        .and_then(|c| c.get("videos"))
        .or_else(|| r.get("videos"));
    if let Some(arr) = raw_videos.and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(url) = v.as_str() {
                video_urls.push(url.to_string());
            } else if let Some(url) = v
                .get("url")
                .or_else(|| v.get("src"))
                .or_else(|| v.get("link"))
                .and_then(|val| val.as_str())
            {
                video_urls.push(url.to_string());
            }
        }
    }
    let videos_count = if video_urls.is_empty() {
        raw_videos
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    } else {
        video_urls.len()
    };

    Some(OzonReviewItem {
        id,
        author,
        date,
        score,
        positive: positive.clone(),
        negative: negative.clone(),
        comment: comment.clone(),
        text: comment,
        useful,
        unuseful,
        likes: useful,
        dislikes: unuseful,
        photos: photos_count,
        photos_count,
        photo_urls,
        videos: videos_count,
        videos_count,
        video_urls,
        item_id,
    })
}

fn parse_aspect_item(a: &Value) -> Option<OzonAspectItem> {
    if !a.is_object() {
        return None;
    }

    let name = a
        .get("title")
        .or_else(|| a.get("name"))
        .or_else(|| a.get("aspect"))
        .or_else(|| a.get("label"))
        .or_else(|| a.get("text"))
        .or_else(|| a.get("description"))
        .and_then(|v| v.as_str())?
        .trim();

    if name.is_empty() {
        return None;
    }

    let score = a
        .get("score")
        .or_else(|| a.get("rating"))
        .or_else(|| a.get("value"))
        .or_else(|| a.get("stars"))
        .and_then(|v| {
            if let Some(f) = v.as_f64() {
                Some(f as f32)
            } else if let Some(s) = v.as_str() {
                s.replace(',', ".").trim().parse::<f32>().ok()
            } else {
                None
            }
        });

    let positive_count = a
        .get("positiveCount")
        .or_else(|| a.get("positive_count"))
        .or_else(|| a.get("positive"))
        .or_else(|| a.get("likes"))
        .or_else(|| a.get("agree"))
        .and_then(|v| v.as_u64());

    let negative_count = a
        .get("negativeCount")
        .or_else(|| a.get("negative_count"))
        .or_else(|| a.get("negative"))
        .or_else(|| a.get("dislikes"))
        .or_else(|| a.get("reject"))
        .and_then(|v| v.as_u64());

    let total_count = a
        .get("totalCount")
        .or_else(|| a.get("total_count"))
        .or_else(|| a.get("total"))
        .or_else(|| a.get("count"))
        .or_else(|| a.get("reviewsCount"))
        .and_then(|v| v.as_u64());

    let sentiment = a
        .get("sentiment")
        .or_else(|| a.get("status"))
        .or_else(|| a.get("type"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(OzonAspectItem {
        name: name.to_string(),
        score,
        positive_count,
        negative_count,
        total_count,
        sentiment,
    })
}
