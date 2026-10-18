use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::models::{
    ParseStatus, YandexCardDetails, YandexProduct, YandexReview, YandexSearchResult,
};

pub const EMPTY_RESULT_MARKER: &str = "Попробуйте сформулировать запрос";
pub const CAPTCHA_MARKERS: &[&str] = &["SmartCaptcha", "/showcaptcha", "checkbox_captcha"];
pub const CARD_SLUG_PLACEHOLDER: &str = "x";
pub const PRODUCT_PAGE_ID: &str = "market:product";
pub const CARD_PRODUCT_COLLECTIONS: &[&str] = &[
    "allPrices",
    "businessRatingStats",
    "mediaItem",
    "offer",
    "price",
    "productServiceSnippets",
    "reviews",
    "shopInfo",
    "titleV2",
];

/// Check if the HTML indicates a real SmartCaptcha challenge.
pub fn looks_like_captcha(html: &str) -> bool {
    CAPTCHA_MARKERS.iter().any(|&marker| html.contains(marker))
}

/// Construct canonical product card URL on Yandex Market.
pub fn card_url(product_id: &str, slug: Option<&str>) -> String {
    let s = slug.unwrap_or(CARD_SLUG_PLACEHOLDER);
    format!("https://market.yandex.ru/card/{s}/{product_id}")
}

/// Unescape basic HTML entities commonly found in data-zone-data and scripts.
pub fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x2F;", "/")
        .replace("&#47;", "/")
}

/// Parse positive finite float, rounding to 2 decimal places.
pub fn to_number(val: Option<&Value>) -> Option<f64> {
    let val = val?;
    let num = match val {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => {
            let clean = s.replace(['\u{00a0}', ' '], "").replace(',', ".");
            clean.parse::<f64>().ok()?
        }
        _ => return None,
    };

    if num <= 0.0 || !num.is_finite() {
        return None;
    }
    Some((num * 100.0).round() / 100.0)
}

/// Parse integer from numeric or string value.
pub fn to_int(val: Option<&Value>) -> Option<u64> {
    to_number(val).map(|n| n as u64)
}

/// Extract all `<noframes data-apiary="patch">(.*?)</noframes>` payloads.
pub fn iter_patches(html: &str) -> Vec<Value> {
    let mut patches = Vec::new();
    let start_tag = r#"<noframes data-apiary="patch">"#;
    let end_tag = "</noframes>";

    let mut cursor = 0;
    while let Some(start_rel) = html[cursor..].find(start_tag) {
        let content_start = cursor + start_rel + start_tag.len();
        if let Some(end_rel) = html[content_start..].find(end_tag) {
            let content_end = content_start + end_rel;
            let json_str = &html[content_start..content_end];
            if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                patches.push(val);
            }
            cursor = content_end + end_tag.len();
        } else {
            break;
        }
    }
    patches
}

/// Extract all `<script type="application/ld+json">(.*?)</script>` payloads.
pub fn iter_ldjson(html: &str) -> Vec<Value> {
    let mut ldjsons = Vec::new();
    let start_tag = r#"<script type="application/ld+json">"#;
    let end_tag = "</script>";

    let mut cursor = 0;
    while let Some(start_rel) = html[cursor..].find(start_tag) {
        let content_start = cursor + start_rel + start_tag.len();
        if let Some(end_rel) = html[content_start..].find(end_tag) {
            let content_end = content_start + end_rel;
            let json_str = &html[content_start..content_end];
            if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                ldjsons.push(val);
            }
            cursor = content_end + end_tag.len();
        } else {
            break;
        }
    }
    ldjsons
}

/// Locate the best product collection bundle on a search page.
pub fn find_search_collections(html: &str) -> Option<serde_json::Map<String, Value>> {
    let mut best: Option<serde_json::Map<String, Value>> = None;
    let mut best_score = 0;

    for patch in iter_patches(html) {
        let widgets = patch.get("widgets").and_then(Value::as_object);
        let Some(widgets) = widgets else { continue };

        for widget_val in widgets.values() {
            let Some(widget_obj) = widget_val.as_object() else {
                continue;
            };

            for path_val in widget_obj.values() {
                let Some(path_obj) = path_val.as_object() else {
                    continue;
                };
                let options = path_obj.get("options").and_then(Value::as_object);
                let Some(options) = options else { continue };
                let collections = options.get("collections").and_then(Value::as_object);
                let Some(collections) = collections else {
                    continue;
                };

                let score = count_map_entries(collections.get("productSnippet"))
                    + count_map_entries(collections.get("offer"))
                    + count_map_entries(collections.get("product"));

                if score > best_score {
                    best_score = score;
                    best = Some(collections.clone());
                }
            }
        }
    }

    best
}

fn count_map_entries(val: Option<&Value>) -> usize {
    val.and_then(Value::as_object).map(|m| m.len()).unwrap_or(0)
}

fn is_empty_value(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        _ => false,
    }
}

/// Merge every top-level collection patch on a product page.
pub fn merge_card_collections(html: &str) -> serde_json::Map<String, Value> {
    let mut merged = serde_json::Map::new();

    for patch in iter_patches(html) {
        let Some(collections) = patch.get("collections").and_then(Value::as_object) else {
            continue;
        };

        for (name, value) in collections {
            let Some(new_map) = value.as_object() else {
                merged.entry(name.clone()).or_insert_with(|| value.clone());
                continue;
            };

            let slot = merged
                .entry(name.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));

            if let Value::Object(slot_map) = slot {
                for (entity_id, entity) in new_map {
                    let existing = slot_map.get(entity_id);
                    if is_empty_value(Some(entity)) && !is_empty_value(existing) {
                        continue;
                    }
                    slot_map.insert(entity_id.clone(), entity.clone());
                }
            }
        }
    }

    merged
}

fn first_dict(val: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    let map = val.and_then(Value::as_object)?;
    for v in map.values() {
        if let Some(inner) = v.as_object() {
            return Some(inner);
        }
    }
    None
}

/// Extract picture URL from a photo node.
pub fn picture_url(node: Option<&Value>) -> String {
    let Some(node) = node.and_then(Value::as_object) else {
        return String::new();
    };

    if let Some(base) = node.get("baseUrl").and_then(Value::as_str)
        && !base.is_empty()
    {
        return format!("{}/orig", base.trim_end_matches('/'));
    }

    let namespace = node.get("namespace").and_then(Value::as_str);
    let group_id = node.get("groupId").and_then(Value::as_str);
    let name = node.get("imageName").and_then(Value::as_str);

    if let (Some(ns), Some(grp), Some(nm)) = (namespace, group_id, name) {
        format!("https://avatars.mds.yandex.net/get-{ns}/{grp}/{nm}/orig")
    } else {
        String::new()
    }
}

/// Convert relative or protocol-relative URL to absolute https URL.
pub fn absolute_url(raw: Option<&str>, product_id: Option<&str>, slug: Option<&str>) -> String {
    if let Some(raw) = raw
        && !raw.is_empty()
    {
        if raw.starts_with("http") {
            return raw.to_string();
        }
        if raw.starts_with("//") {
            return format!("https:{raw}");
        }
        return format!("https://market.yandex.ru{raw}");
    }

    if let Some(pid) = product_id
        && !pid.is_empty()
    {
        return card_url(pid, slug);
    }

    String::new()
}

#[derive(Debug, Default, Clone)]
pub struct LdJsonItem {
    pub product_id: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub sku_id: Option<String>,
    pub price_with_plus: Option<f64>,
    pub rating: Option<f32>,
    pub rating_count: Option<u32>,
}

/// Extract schema.org ItemList from search HTML.
pub fn ldjson_item_list(html: &str) -> Vec<LdJsonItem> {
    let mut items = Vec::new();

    for obj in iter_ldjson(html) {
        if obj.get("@type").and_then(Value::as_str) != Some("ItemList") {
            continue;
        }

        let Some(elements) = obj.get("itemListElement").and_then(Value::as_array) else {
            continue;
        };

        for el in elements {
            let item = el.get("item").and_then(Value::as_object);
            let Some(item) = item else { continue };

            let offers = item.get("offers").and_then(Value::as_object);
            let rating = item.get("aggregateRating").and_then(Value::as_object);
            let url_str = item.get("url").and_then(Value::as_str).unwrap_or_default();

            let product_id = if !url_str.is_empty() {
                url_str
                    .trim_end_matches('/')
                    .split('/')
                    .next_back()
                    .map(str::to_string)
            } else {
                None
            };

            items.push(LdJsonItem {
                product_id,
                title: item.get("name").and_then(Value::as_str).map(str::to_string),
                url: if !url_str.is_empty() {
                    Some(url_str.to_string())
                } else {
                    None
                },
                image: item
                    .get("image")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sku_id: item.get("sku").and_then(Value::as_str).map(str::to_string),
                price_with_plus: offers.and_then(|o| to_number(o.get("price"))),
                rating: rating.and_then(|r| to_number(r.get("ratingValue")).map(|n| n as f32)),
                rating_count: rating.and_then(|r| to_int(r.get("ratingCount")).map(|n| n as u32)),
            });
        }
    }

    items
}

#[derive(Debug, Default, Clone)]
pub struct LdJsonProduct {
    pub title: Option<String>,
    pub brand: Option<String>,
    pub image: Option<String>,
    pub description: Option<String>,
    pub sku: Option<String>,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub url: Option<String>,
}

/// Extract schema.org Product from card HTML.
pub fn ldjson_product(html: &str) -> Option<LdJsonProduct> {
    for obj in iter_ldjson(html) {
        if obj.get("@type").and_then(Value::as_str) != Some("Product") {
            continue;
        }

        let offers = obj.get("offers").and_then(Value::as_object);

        return Some(LdJsonProduct {
            title: obj.get("name").and_then(Value::as_str).map(str::to_string),
            brand: obj.get("brand").and_then(Value::as_str).map(str::to_string),
            image: obj.get("image").and_then(Value::as_str).map(str::to_string),
            description: obj
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            sku: obj.get("sku").and_then(Value::as_str).map(str::to_string),
            price: offers.and_then(|o| to_number(o.get("price"))),
            currency: offers
                .and_then(|o| o.get("priceCurrency").and_then(Value::as_str))
                .map(str::to_string),
            url: obj.get("url").and_then(Value::as_str).map(str::to_string),
        });
    }
    None
}

/// Extract raw JSON payloads out of `data-zone-data` attributes on product snippets.
pub fn zone_snippets(html: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let needle = r#"data-zone-data=""#;
    let mut cursor = 0;

    while let Some(pos) = html[cursor..].find(needle) {
        let val_start = cursor + pos + needle.len();
        if let Some(quote_end) = html[val_start..].find('"') {
            let raw_val = &html[val_start..val_start + quote_end];
            let unescaped = unescape_html(raw_val);
            if let Ok(val) = serde_json::from_str::<Value>(&unescaped)
                && val.get("type").and_then(Value::as_str) == Some("offer")
            {
                out.push(val);
            }
            cursor = val_start + quote_end + 1;
        } else {
            break;
        }
    }

    out
}

fn zone_shop_name(payload: &Value) -> Option<String> {
    let signals = payload.get("signals").and_then(Value::as_array)?;
    for sig in signals {
        if sig.get("type").and_then(Value::as_str) == Some("shop")
            && let Some(title) = sig.get("title").and_then(Value::as_str)
            && !title.is_empty()
        {
            return Some(title.to_string());
        }
    }
    None
}

fn additional_price(payload: Option<&Value>, price_type: &str) -> Option<f64> {
    let payload = payload?;
    let prices = payload.get("additionalPrices").and_then(Value::as_array)?;
    for entry in prices {
        if entry.get("priceType").and_then(Value::as_str) == Some(price_type) {
            return to_number(entry.get("priceValue"));
        }
    }
    None
}

fn amount_int(node: Option<&Value>) -> Option<f64> {
    let amount = node?.get("amount")?;
    to_number(amount.get("intPart"))
}

/// Parse first-screen snippet zone payloads into YandexProducts.
pub fn parse_zone_items(html: &str) -> Vec<YandexProduct> {
    let payloads = zone_snippets(html);
    if payloads.is_empty() {
        return Vec::new();
    }

    let ldjson_items = ldjson_item_list(html);
    let ldjson_by_id: HashMap<String, LdJsonItem> = ldjson_items
        .into_iter()
        .filter_map(|it| it.product_id.clone().map(|id| (id, it)))
        .collect();

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for payload in &payloads {
        let osku_id = payload.get("oskuId").and_then(Value::as_str);
        let model_id = payload.get("modelId").and_then(Value::as_str);
        let product_id = osku_id.or(model_id).unwrap_or_default().to_string();

        let market_sku = payload
            .get("marketSku")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ware_id = payload
            .get("wareId")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let key = (
            product_id.clone(),
            market_sku.to_string(),
            ware_id.to_string(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let base_price = to_number(payload.get("price"));
        let with_discount = additional_price(Some(payload), "withDiscount");
        let price_rub = with_discount.or(base_price);
        let price_old = if let (Some(bp), Some(pr)) = (base_price, price_rub) {
            if bp > pr { Some(bp) } else { None }
        } else {
            None
        };

        let price_with_plus = additional_price(Some(payload), "yaBank");
        let rating_node = payload.get("rating");
        let is_available = payload.get("isAvailable").and_then(Value::as_bool);
        let is_express = payload
            .get("isExpress")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let ld = ldjson_by_id.get(&product_id);

        let title = payload
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| ld.and_then(|l| l.title.clone()))
            .unwrap_or_default();

        let url = ld
            .and_then(|l| l.url.clone())
            .unwrap_or_else(|| absolute_url(None, Some(&product_id), None));

        let image = ld.and_then(|l| l.image.clone()).unwrap_or_default();

        let rating = rating_node
            .and_then(|r| to_number(r.get("rating")).map(|n| n as f32))
            .or_else(|| ld.and_then(|l| l.rating));

        let rating_count = rating_node
            .and_then(|r| to_int(r.get("gradesCount")).map(|n| n as u32))
            .or_else(|| ld.and_then(|l| l.rating_count));

        items.push(YandexProduct {
            product_id: if !product_id.is_empty() {
                Some(product_id)
            } else {
                None
            },
            sku_id: if !market_sku.is_empty() {
                Some(market_sku.to_string())
            } else {
                None
            },
            title,
            brand: None,
            seller: zone_shop_name(payload),
            price_rub,
            price_with_plus,
            price_old_rub: price_old,
            currency: "RUR".to_string(),
            rating,
            rating_count,
            in_stock: is_available,
            is_express,
            url,
            image,
            source: "zone".to_string(),
        });
    }

    items
}

/// Parse search page HTML into structured YandexSearchResult.
pub fn parse_search(html: &str) -> YandexSearchResult {
    if looks_like_captcha(html) {
        return YandexSearchResult {
            status: ParseStatus::Captcha,
            query: String::new(),
            total: None,
            page: None,
            page_count: None,
            has_next_page: false,
            items: Vec::new(),
        };
    }

    let collections = find_search_collections(html);
    let source = collections
        .clone()
        .unwrap_or_else(|| merge_card_collections(html));

    let visible = first_dict(source.get("visibleSearchResult"));
    let query = visible
        .and_then(|v| v.get("text").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let total = visible.and_then(|v| to_int(v.get("total")));
    let page = visible.and_then(|v| to_int(v.get("page")).map(|n| n as u32));
    let page_count = visible.and_then(|v| to_int(v.get("pageCount")).map(|n| n as u32));
    let has_next_page = visible
        .and_then(|v| v.get("hasNextPage").and_then(Value::as_bool))
        .unwrap_or(false);

    if collections.is_none() {
        // Fallback: try zone items
        let zone_items = parse_zone_items(html);
        if !zone_items.is_empty() {
            let count = zone_items.len() as u64;
            return YandexSearchResult {
                status: ParseStatus::OkZone,
                query,
                total: total.or(Some(count)),
                page: page.or(Some(1)),
                page_count: page_count.or(Some(1)),
                has_next_page,
                items: zone_items,
            };
        }

        // Fallback: try schema.org ld+json
        let ld_items = ldjson_item_list(html);
        if !ld_items.is_empty() {
            let items: Vec<YandexProduct> = ld_items
                .into_iter()
                .map(|ld| YandexProduct {
                    product_id: ld.product_id,
                    sku_id: ld.sku_id,
                    title: ld.title.unwrap_or_default(),
                    brand: None,
                    seller: None,
                    price_rub: ld.price_with_plus,
                    price_with_plus: ld.price_with_plus,
                    price_old_rub: None,
                    currency: "RUR".to_string(),
                    rating: ld.rating,
                    rating_count: ld.rating_count,
                    in_stock: Some(true),
                    is_express: false,
                    url: ld.url.unwrap_or_default(),
                    image: ld.image.unwrap_or_default(),
                    source: "ld+json".to_string(),
                })
                .collect();

            let count = items.len() as u64;
            return YandexSearchResult {
                status: ParseStatus::OkLdjsonOnly,
                query,
                total: total.or(Some(count)),
                page: page.or(Some(1)),
                page_count: page_count.or(Some(1)),
                has_next_page,
                items,
            };
        }

        let status = if html.contains(EMPTY_RESULT_MARKER) {
            ParseStatus::Empty
        } else {
            ParseStatus::NoProductsFound
        };

        return YandexSearchResult {
            status,
            query,
            total,
            page,
            page_count,
            has_next_page,
            items: Vec::new(),
        };
    }

    let collections = collections.unwrap();
    let offers = collections.get("offer").and_then(Value::as_object);
    let products = collections.get("product").and_then(Value::as_object);
    let snippets = collections.get("productSnippet").and_then(Value::as_object);
    let shops = collections.get("shop").and_then(Value::as_object);
    let vendors = collections.get("vendor").and_then(Value::as_object);
    let show_places = collections
        .get("productShowPlace")
        .and_then(Value::as_object);
    let offer_places = collections.get("offerShowPlace").and_then(Value::as_object);
    let visible_entities = collections.get("visibleEntity").and_then(Value::as_object);
    let search_results = collections.get("searchResult").and_then(Value::as_object);

    // Resolve order from visibleEntityIds if present
    let mut ordered_places: Vec<&serde_json::Map<String, Value>> = Vec::new();
    if let (Some(sr_map), Some(ve_map), Some(sp_map)) =
        (search_results, visible_entities, show_places)
    {
        for sr in sr_map.values() {
            if let Some(ids) = sr.get("visibleEntityIds").and_then(Value::as_array) {
                for id_val in ids {
                    if let Some(id_str) = id_val.as_str()
                        && id_str.starts_with("showPlace_")
                        && let Some(entity) = ve_map.get(id_str).and_then(Value::as_object)
                        && let Some(sp_id) =
                            entity.get("productShowPlaceId").and_then(Value::as_str)
                        && let Some(place) = sp_map.get(sp_id).and_then(Value::as_object)
                    {
                        ordered_places.push(place);
                    }
                }
            }
        }
    }

    let places = if !ordered_places.is_empty() {
        ordered_places
    } else if let Some(sp_map) = show_places {
        sp_map.values().filter_map(Value::as_object).collect()
    } else {
        Vec::new()
    };

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for place in places {
        let product_id = place.get("productId").and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        });

        let snippet_id = place
            .get("productSnippetId")
            .and_then(Value::as_str)
            .map(str::to_string);

        let place_id = place
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();

        let key = snippet_id
            .clone()
            .unwrap_or_else(|| format!("{}_{place_id}", product_id.as_deref().unwrap_or_default()));

        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let snippet = snippet_id
            .as_deref()
            .and_then(|id| snippets.and_then(|s| s.get(id)))
            .and_then(Value::as_object);

        let payload = snippet.and_then(|s| s.get("productPayload"));
        let payload_obj = payload.and_then(Value::as_object);
        let baobab = snippet.and_then(|s| s.get("baobabPayload"));
        let baobab_obj = baobab.and_then(Value::as_object);

        // Resolve offer
        let offer_from_place = place
            .get("defaultOfferShowPlaceId")
            .and_then(Value::as_str)
            .and_then(|op_id| offer_places.and_then(|op| op.get(op_id)))
            .and_then(Value::as_object)
            .and_then(|op| op.get("offerId"))
            .and_then(Value::as_str)
            .and_then(|o_id| offers.and_then(|o| o.get(o_id)))
            .and_then(Value::as_object);

        let offer = offer_from_place.or_else(|| {
            if let Some(sn_id) = &snippet_id
                && let Some(key) = sn_id.strip_prefix("prime-")
            {
                return offers.and_then(|o| o.get(key)).and_then(Value::as_object);
            }
            None
        });

        let product = product_id
            .as_deref()
            .and_then(|pid| products.and_then(|p| p.get(pid)))
            .and_then(Value::as_object);

        let price_node = payload_obj
            .and_then(|p| p.get("price"))
            .and_then(Value::as_object);

        let cart_price = payload_obj
            .and_then(|p| p.get("cartButton"))
            .and_then(|b| b.get("price"))
            .and_then(|pr| to_number(pr.get("valueFmt")));

        let with_discount = additional_price(baobab, "withDiscount");
        let offer_price = offer
            .and_then(|o| o.get("price"))
            .and_then(|pr| to_number(pr.get("value")));

        let price_rub = cart_price.or(with_discount).or(offer_price);

        let price_with_plus = amount_int(price_node.and_then(|p| p.get("actualPrice")))
            .or_else(|| additional_price(baobab, "yaBank"));

        let price_old = amount_int(price_node.and_then(|p| p.get("oldPrice")))
            .or_else(|| amount_int(price_node.and_then(|p| p.get("initialPrice"))))
            .or_else(|| {
                if let (Some(bp), Some(pr)) = (
                    baobab_obj.and_then(|b| to_number(b.get("price"))),
                    price_rub,
                ) {
                    if bp > pr { Some(bp) } else { None }
                } else {
                    None
                }
            });

        let rating_node = payload_obj
            .and_then(|p| p.get("rating"))
            .and_then(Value::as_object);
        let delivery = offer
            .and_then(|o| o.get("delivery"))
            .and_then(Value::as_object);

        let vendor_id = offer
            .and_then(|o| o.get("vendorId"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty());

        let supplier_id = offer
            .and_then(|o| o.get("supplierId"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty());

        let vendor = vendor_id
            .as_deref()
            .and_then(|vid| vendors.and_then(|v| v.get(vid)))
            .and_then(Value::as_object);

        let shop = supplier_id
            .as_deref()
            .and_then(|sid| shops.and_then(|s| s.get(sid)))
            .and_then(Value::as_object);

        let brand = vendor
            .and_then(|v| v.get("name").and_then(Value::as_str))
            .map(str::to_string);

        let seller = shop
            .and_then(|s| s.get("name").and_then(Value::as_str))
            .map(str::to_string);

        let titles = product
            .and_then(|p| p.get("titles"))
            .and_then(Value::as_object);
        let offer_titles = offer
            .and_then(|o| o.get("titles"))
            .and_then(Value::as_object);
        let title_node = payload_obj
            .and_then(|p| p.get("title"))
            .and_then(Value::as_object);

        let title = title_node
            .and_then(|t| t.get("value").and_then(Value::as_str))
            .or_else(|| titles.and_then(|t| t.get("raw").and_then(Value::as_str)))
            .or_else(|| offer_titles.and_then(|t| t.get("raw").and_then(Value::as_str)))
            .unwrap_or_default()
            .to_string();

        let direct_url = place
            .get("urls")
            .and_then(Value::as_object)
            .and_then(|u| u.get("direct"))
            .and_then(Value::as_str);

        let product_slug = product.and_then(|p| p.get("slug")).and_then(Value::as_str);

        let url = absolute_url(direct_url, product_id.as_deref(), product_slug);

        let media = payload_obj
            .and_then(|p| p.get("gallery"))
            .and_then(|g| g.get("mediaItems"))
            .and_then(Value::as_array);

        let first_media = media.and_then(|m| m.first());
        let image = picture_url(first_media.and_then(|fm| fm.get("picture")));

        let rating = rating_node.and_then(|r| to_number(r.get("ratingValue")).map(|n| n as f32));
        let rating_count = rating_node.and_then(|r| to_int(r.get("ratingCount")).map(|n| n as u32));

        let in_stock = delivery.and_then(|d| d.get("inStock").and_then(Value::as_bool));
        let is_express = delivery
            .and_then(|d| d.get("isExpress").and_then(Value::as_bool))
            .unwrap_or(false);

        let currency = offer
            .and_then(|o| o.get("price"))
            .and_then(|p| p.get("currency").and_then(Value::as_str))
            .unwrap_or("RUR")
            .to_string();

        let sku_id = offer
            .and_then(|o| o.get("skuId"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty());

        items.push(YandexProduct {
            product_id,
            sku_id,
            title,
            brand,
            seller,
            price_rub,
            price_with_plus,
            price_old_rub: price_old,
            currency,
            rating,
            rating_count,
            in_stock,
            is_express,
            url,
            image,
            source: "ssr".to_string(),
        });
    }

    let status = if items.is_empty() {
        if html.contains(EMPTY_RESULT_MARKER) {
            ParseStatus::Empty
        } else {
            ParseStatus::NoProductsFound
        }
    } else {
        ParseStatus::Ok
    };

    YandexSearchResult {
        status,
        query,
        total,
        page,
        page_count,
        has_next_page,
        items,
    }
}

/// Extract customer reviews from merged product page collection state.
pub fn parse_reviews(merged: &serde_json::Map<String, Value>) -> Vec<YandexReview> {
    let reviews_col = merged.get("reviews").and_then(Value::as_object);
    let Some(reviews_col) = reviews_col else {
        return Vec::new();
    };

    let Some(raw_reviews) = reviews_col.get("reviews").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();

    for raw in raw_reviews {
        let Some(raw_obj) = raw.as_object() else {
            continue;
        };

        let author = raw_obj
            .get("author")
            .and_then(Value::as_object)
            .and_then(|a| a.get("nickname").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();

        let rating = to_int(raw_obj.get("rating")).map(|n| n as u8);

        let mut date = String::new();
        if let Some(desc_list) = raw_obj.get("descriptor").and_then(Value::as_array) {
            for d in desc_list {
                if d.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(content) = d.get("content").and_then(Value::as_str)
                {
                    date = content.to_string();
                    break;
                }
            }
        }

        let pros = raw_obj
            .get("pro")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let cons = raw_obj
            .get("contra")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let comment = raw_obj
            .get("comment")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let votes = raw_obj.get("votes").and_then(Value::as_object);
        let votes_up = votes
            .and_then(|v| to_int(v.get("votesAgree")).map(|n| n as i32))
            .unwrap_or(0);
        let votes_down = votes
            .and_then(|v| to_int(v.get("votesReject")).map(|n| n as i32))
            .unwrap_or(0);

        let mut photos = Vec::new();
        if let Some(media_list) = raw_obj.get("media").and_then(Value::as_array) {
            for m in media_list {
                let purl = picture_url(m.get("picture"));
                if !purl.is_empty() {
                    photos.push(purl);
                }
            }
        }

        out.push(YandexReview {
            author,
            rating,
            date,
            pros,
            cons,
            comment,
            votes_up,
            votes_down,
            photos,
        });
    }

    out
}

fn empty_product_shell(merged: &serde_json::Map<String, Value>, has_fallback: bool) -> bool {
    let params = first_dict(merged.get("pageParams"));
    if params.and_then(|p| p.get("pageId").and_then(Value::as_str)) != Some(PRODUCT_PAGE_ID) {
        return false;
    }
    if has_fallback {
        return false;
    }

    CARD_PRODUCT_COLLECTIONS.iter().all(|&name| {
        let val = merged.get(name);
        count_map_entries(val) == 0
    })
}

fn shell_product_id(merged: &serde_json::Map<String, Value>) -> String {
    let params_dict = first_dict(merged.get("pageParams"));
    let inner_params = params_dict
        .and_then(|p| p.get("params"))
        .and_then(Value::as_object);

    inner_params
        .and_then(|ip| ip.get("productId"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

fn stars_from_stats(stats: Option<&Value>) -> HashMap<u8, u32> {
    let mut out = HashMap::new();
    let Some(stats) = stats.and_then(Value::as_object) else {
        return out;
    };

    for star in 1..=5 {
        let key = format!("cnt{star}");
        let val = to_int(stats.get(&key)).unwrap_or(0) as u32;
        out.insert(star, val);
    }
    out
}

fn stars_from_distribution(distribution: Option<&Value>) -> HashMap<u8, u32> {
    let mut out = HashMap::new();
    let Some(parts) = distribution
        .and_then(Value::as_object)
        .and_then(|d| d.get("parts"))
        .and_then(Value::as_array)
    else {
        return out;
    };

    for part in parts {
        let star = to_int(part.get("value")).map(|n| n as u8);
        let count = to_int(part.get("number")).unwrap_or(0) as u32;
        if let Some(s) = star
            && (1..=5).contains(&s)
        {
            out.insert(s, count);
        }
    }
    out
}

/// Parse a product page HTML into full product details, ratings, and reviews.
pub fn parse_card(html: &str) -> YandexCardDetails {
    if looks_like_captcha(html) {
        return YandexCardDetails {
            status: ParseStatus::Captcha,
            product_id: String::new(),
            sku_id: None,
            title: String::new(),
            brand: None,
            description: None,
            image: None,
            seller: None,
            price_rub: None,
            price_with_plus: None,
            price_before_discount_rub: None,
            discount_percent: None,
            currency: "RUR".to_string(),
            offers_count: None,
            rating: None,
            rating_count: None,
            review_count: None,
            rating_stars: HashMap::new(),
            reviews: Vec::new(),
        };
    }

    let merged = merge_card_collections(html);
    let fallback = ldjson_product(html);

    let all_prices = first_dict(merged.get("allPrices"));
    let baobab = all_prices
        .and_then(|ap| ap.get("baobab"))
        .and_then(Value::as_object);

    let title_node = first_dict(merged.get("titleV2"));
    let vendor_node = title_node
        .and_then(|tn| tn.get("vendor"))
        .and_then(Value::as_object);

    let mut product_id = baobab
        .and_then(|b| b.get("productId"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();

    if product_id.is_empty()
        && let Some(fb_url) = fallback.as_ref().and_then(|f| f.url.as_deref())
    {
        // Extract numeric ID from URL e.g. /product--.../763970960?sku=... or /card/.../763970960
        let path_part = fb_url.split('?').next().unwrap_or(fb_url);
        let digits: String = path_part
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            product_id = digits;
        }
    }

    let price_node = first_dict(merged.get("price"));
    let main_price = price_node
        .and_then(|pn| pn.get("mainPrice"))
        .and_then(Value::as_object);

    let price_with_plus = main_price
        .and_then(|mp| mp.get("price"))
        .and_then(|p| to_number(p.get("value")))
        .or_else(|| fallback.as_ref().and_then(|f| f.price));

    let mut price_regular = None;
    let mut price_before_discount = None;

    if let Some(old_prices) = price_node
        .and_then(|pn| pn.get("oldPrices"))
        .and_then(Value::as_array)
    {
        for old in old_prices {
            let val = old.get("price").and_then(|p| to_number(p.get("value")));
            match old.get("type").and_then(Value::as_str) {
                Some("regular") => price_regular = val,
                Some("withoutDiscount") => price_before_discount = val,
                _ => {}
            }
        }
    }

    let snippet = first_dict(merged.get("productServiceSnippets"));
    let product_snippet = snippet
        .and_then(|s| s.get("productSnippet"))
        .and_then(Value::as_object);
    let snippet_payload = product_snippet
        .and_then(|ps| ps.get("productPayload"))
        .and_then(Value::as_object);
    let snippet_rating = snippet_payload
        .and_then(|sp| sp.get("rating"))
        .and_then(Value::as_object);

    let reviews_col = merged.get("reviews").and_then(Value::as_object);
    let rating_stats = reviews_col
        .and_then(|rc| rc.get("ratingStats"))
        .and_then(Value::as_object);
    let review_stats = reviews_col
        .and_then(|rc| rc.get("reviewStats"))
        .and_then(Value::as_object);

    let business_stats = first_dict(merged.get("businessRatingStats"));
    let shop_info = first_dict(merged.get("shopInfo"));

    let rating = snippet_rating
        .and_then(|sr| to_number(sr.get("ratingValue")).map(|n| n as f32))
        .or_else(|| {
            rating_stats
                .and_then(|rs| to_number(rs.get("ratingValue")).map(|n| n as f32))
                .or_else(|| {
                    rating_stats.and_then(|rs| to_number(rs.get("roundedRating")).map(|n| n as f32))
                })
        })
        .or_else(|| {
            business_stats.and_then(|bs| to_number(bs.get("ratingValue")).map(|n| n as f32))
        });

    let rating_count = snippet_rating
        .and_then(|sr| to_int(sr.get("ratingCount")).map(|n| n as u32))
        .or_else(|| rating_stats.and_then(|rs| to_int(rs.get("ratingCount")).map(|n| n as u32)))
        .or_else(|| business_stats.and_then(|bs| to_int(bs.get("ratingCount")).map(|n| n as u32)));

    let mut stars = stars_from_stats(snippet_rating.and_then(|sr| sr.get("ratingCountStats")));
    if stars.is_empty() {
        stars = stars_from_distribution(reviews_col.and_then(|rc| rc.get("distribution")));
    }

    let sku_id = baobab
        .and_then(|b| b.get("skuId"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .or_else(|| fallback.as_ref().and_then(|f| f.sku.clone()));

    let title = title_node
        .and_then(|tn| tn.get("title").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| fallback.as_ref().and_then(|f| f.title.clone()))
        .unwrap_or_default();

    let brand = vendor_node
        .and_then(|vn| vn.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| fallback.as_ref().and_then(|f| f.brand.clone()));

    let description = fallback.as_ref().and_then(|f| f.description.clone());
    let image = fallback.as_ref().and_then(|f| f.image.clone());

    let seller = shop_info
        .and_then(|si| {
            si.get("shopName")
                .or_else(|| si.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .or_else(|| {
            business_stats
                .and_then(|bs| bs.get("shopName").and_then(Value::as_str))
                .map(str::to_string)
        });

    let price_rub = price_regular.or(price_with_plus);
    let discount_percent =
        price_node.and_then(|pn| to_int(pn.get("discountPercent")).map(|n| n as u32));

    let currency = main_price
        .and_then(|mp| mp.get("price"))
        .and_then(|p| p.get("currency").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| fallback.as_ref().and_then(|f| f.currency.clone()))
        .unwrap_or_else(|| "RUR".to_string());

    let offers_count = baobab.and_then(|b| to_int(b.get("numOffers")).map(|n| n as u32));

    let review_count = snippet_rating
        .and_then(|sr| to_int(sr.get("reviewsCount")).map(|n| n as u32))
        .or_else(|| review_stats.and_then(|rs| to_int(rs.get("reviewsCount")).map(|n| n as u32)));

    let reviews = parse_reviews(&merged);

    let mut result = YandexCardDetails {
        status: ParseStatus::Ok,
        product_id,
        sku_id,
        title,
        brand,
        description,
        image,
        seller,
        price_rub,
        price_with_plus,
        price_before_discount_rub: price_before_discount,
        discount_percent,
        currency,
        offers_count,
        rating,
        rating_count,
        review_count,
        rating_stars: stars,
        reviews,
    };

    if result.title.is_empty()
        && result.price_rub.is_none()
        && result.price_with_plus.is_none()
        && empty_product_shell(&merged, fallback.is_some())
    {
        result.status = ParseStatus::EmptyProductShell;
        if result.product_id.is_empty() {
            result.product_id = shell_product_id(&merged);
        }
    }

    result
}
