use scraper::{Html, Selector};
use std::collections::HashSet;

use ru_marketplace_core::domain::{ScraperError, WafProtection};

use crate::models::{TaobaoCardDetails, TaobaoSearchItem};

pub const TAOBAO_SEARCH_BASE: &str = "https://s.taobao.com/search";
pub const TAOBAO_ITEM_BASE: &str = "https://item.taobao.com/item.htm";

/// Ceiling on anchor count for login-wall structural detection.
const WALL_MAX_ANCHORS: usize = 40;

/// Construct canonical item card URL.
pub fn canonical_item_url(item_id: &str) -> String {
    format!("{TAOBAO_ITEM_BASE}?id={item_id}")
}

/// Construct canonical search URL.
pub fn canonical_search_url(query: &str, page: usize) -> String {
    let mut url = reqwest::Url::parse(TAOBAO_SEARCH_BASE)
        .unwrap_or_else(|_| reqwest::Url::parse("https://s.taobao.com/search").unwrap());
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", query);
        if page > 1 {
            pairs.append_pair("page", &page.to_string());
        }
    }
    url.to_string()
}

/// Extract numeric Taobao item ID (9-13 digits) from an item URL or bare ID string.
pub fn extract_item_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.chars().all(|c| c.is_ascii_digit()) && (9..=13).contains(&raw.len()) {
        return Some(raw.to_string());
    }

    let url_to_parse = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_string()
    };

    if let Ok(url) = reqwest::Url::parse(&url_to_parse) {
        if let Some(host) = url.host_str() {
            let host = host.trim_end_matches('.').to_lowercase();
            if host != "taobao.com" && !host.ends_with(".taobao.com") {
                return None;
            }
        }
        for (k, v) in url.query_pairs() {
            if k == "id" && v.chars().all(|c| c.is_ascii_digit()) && (9..=13).contains(&v.len()) {
                return Some(v.into_owned());
            }
        }
    }

    // Fallback: look for id=(\d{9,13}) anywhere in string
    if let Some(pos) = raw.find("id=") {
        let after = &raw[pos + 3..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if (9..=13).contains(&digits.len()) {
            return Some(digits);
        }
    }

    None
}

/// Parse CNY float price from a price string, e.g. "999¥", "¥7999", "66.8".
/// Returns None if the string does not represent a valid price (e.g. "面议").
pub fn parse_cny_price(text: &str) -> Option<f64> {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().position(|c| c.is_ascii_digit())?;

    let mut num_str = String::new();
    let mut has_dot = false;
    for &c in &chars[start..] {
        if c.is_ascii_digit() {
            num_str.push(c);
        } else if c == '.' && !has_dot {
            has_dot = true;
            num_str.push(c);
        } else {
            break;
        }
    }

    num_str.parse::<f64>().ok().filter(|&p| p > 0.0)
}

/// Parse sales count from text such as "2000+人付款" -> Some(2000), "约售 12 件" -> Some(12).
pub fn parse_sales_count(text: &str) -> Option<u32> {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u32>().ok()
}

/// Check whether the document represents a Login Wall or anti-bot challenge.
pub fn check_login_wall_or_challenge(document: &Html) -> Result<(), ScraperError> {
    let title_sel = Selector::parse("title").unwrap();
    let doc_title = document
        .select(&title_sel)
        .next()
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    let doc_title_lower = doc_title.to_lowercase();
    if doc_title.contains("登录")
        || doc_title_lower.contains("login")
        || doc_title_lower.contains("sign in")
    {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Kasada,
            details: format!("Taobao login wall triggered (document title: '{doc_title}')"),
        });
    }

    if doc_title == "__BLOCKED__" {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Kasada,
            details: "Taobao challenge blocked page (__BLOCKED__)".to_string(),
        });
    }

    // Structural login-wall markers on link-poor pages (variant captured 2026-09-10)
    let a_sel = Selector::parse("a[href]").unwrap();
    let anchors_total = document.select(&a_sel).count();

    if anchors_total < WALL_MAX_ANCHORS {
        let login_route_sel = Selector::parse(
            "a[href*='login.jhtml'], a[href*='register.jhtml'], a[href*='new_register.jhtml']",
        )
        .unwrap();
        let login_anchors = document.select(&login_route_sel).count();
        if login_anchors > 0 {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!(
                    "Taobao structural login wall triggered ({login_anchors} login routes, {anchors_total} total anchors)"
                ),
            });
        }

        let body_sel = Selector::parse("body").unwrap();
        let body_text = document
            .select(&body_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();

        let body_lower = body_text.to_lowercase();
        if body_text.contains("登录")
            || body_text.contains("免费注册")
            || body_lower.contains("log in")
            || body_lower.contains("sign in")
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: format!(
                    "Taobao login wall triggered (body login markers on link-poor page with {anchors_total} anchors)"
                ),
            });
        }
    }

    Ok(())
}

/// Parse HTML search results into a list of `TaobaoSearchItem`s.
pub fn parse_search_html(html: &str, limit: usize) -> Result<Vec<TaobaoSearchItem>, ScraperError> {
    let document = Html::parse_document(html);

    // 1. Detect login wall or anti-bot challenge
    check_login_wall_or_challenge(&document)?;

    let mut items = Vec::new();
    let mut seen_ids = HashSet::new();

    // Selectors for card tiles: both modeled fixture and modern live React DOM
    let card_sel = Selector::parse(
        "[class*='search-content-col'], [class*='Card--doubleCard'], a[class*='doubleCardWrapperAdapt'], a[href*='item.taobao.com'], div[class*='doubleCard--']",
    )
    .unwrap();
    let anchor_sel =
        Selector::parse("a[href*='item.taobao.com'], a[href*='id='], a[data-spm-act-id]").unwrap();
    let title_sel = Selector::parse(
        "[class*='Title--title'], [class*='title--'], [class*='Title'], [class*='title'], a[title]",
    )
    .unwrap();
    let price_int_sel = Selector::parse("[class*='priceInt']").unwrap();
    let price_float_sel = Selector::parse("[class*='priceFloat']").unwrap();
    let price_text_sel = Selector::parse(
        "[class*='priceText'], [class*='Price--priceText'], [class*='PriceWrapper'], [class*='priceWrapper']",
    )
    .unwrap();
    let shop_sel = Selector::parse(
        "[class*='Shop--shop'], [class*='shopNameText'], [class*='Shop--'], [class*='shop--'], [class*='shopName']",
    )
    .unwrap();
    let sales_sel = Selector::parse(
        "[class*='realSales'], [class*='Sales--sales'], [class*='sales'], [class*='Sales']",
    )
    .unwrap();
    let procity_sel = Selector::parse("[class*='procity']").unwrap();

    for card in document.select(&card_sel) {
        if items.len() >= limit {
            break;
        }

        // 1. Item ID and URL
        let mut item_id = None;

        if let Some(href) = card.value().attr("href") {
            item_id = extract_item_id(href);
        }

        if item_id.is_none() {
            for a in card.select(&anchor_sel) {
                if let Some(href) = a.value().attr("href")
                    && let Some(id) = extract_item_id(href)
                {
                    item_id = Some(id);
                    break;
                }
            }
        }

        if item_id.is_none()
            && let Some(id_attr) = card.value().attr("id")
            && let Some(stripped) = id_attr.strip_prefix("item_id_")
        {
            item_id = extract_item_id(stripped);
        }

        if item_id.is_none()
            && let Some(act_id) = card.value().attr("data-spm-act-id")
        {
            item_id = extract_item_id(act_id);
        }

        let item_id = match item_id {
            Some(id) => id,
            None => continue,
        };

        if !seen_ids.insert(item_id.clone()) {
            continue;
        }

        // 2. Title
        let mut title = None;
        for t_elem in card.select(&title_sel) {
            if let Some(attr_t) = t_elem.value().attr("title") {
                let trimmed = attr_t.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.to_string());
                    break;
                }
            }
            let text = t_elem.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                title = Some(text);
                break;
            }
        }

        if title.is_none() {
            for a in card.select(&anchor_sel) {
                let text = a.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    title = Some(text);
                    break;
                }
            }
        }

        let title = match title {
            Some(t) => t,
            None => continue,
        };

        // 3. Price in CNY
        let price_cny = if let Some(int_elem) = card.select(&price_int_sel).next() {
            let int_text = int_elem.text().collect::<String>().trim().to_string();
            let float_text = card
                .select(&price_float_sel)
                .next()
                .map(|e| e.text().collect::<String>())
                .unwrap_or_default();
            let full_price = format!("{int_text}{float_text}");
            parse_cny_price(&full_price)
        } else if let Some(price_elem) = card.select(&price_text_sel).next() {
            let p_text = price_elem.text().collect::<String>();
            parse_cny_price(&p_text)
        } else {
            None
        };

        // 4. Shop Name
        let mut shop_name = None;
        if let Some(shop_elem) = card.select(&shop_sel).next() {
            let s_text = shop_elem.text().collect::<String>().trim().to_string();
            if !s_text.is_empty() {
                shop_name = Some(s_text);
            } else if let Some(attr_t) = shop_elem.value().attr("title") {
                let trimmed = attr_t.trim();
                if !trimmed.is_empty() {
                    shop_name = Some(trimmed.to_string());
                }
            }
        }

        // 5. Sales
        let mut sales = None;
        let mut sales_count = None;
        if let Some(sales_elem) = card.select(&sales_sel).next() {
            let s_text = sales_elem.text().collect::<String>().trim().to_string();
            if !s_text.is_empty() {
                sales_count = parse_sales_count(&s_text);
                sales = Some(s_text);
            }
        }

        // 6. Location
        let mut location_parts = Vec::new();
        for p_elem in card.select(&procity_sel) {
            let text = p_elem.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                location_parts.push(text);
            }
        }
        let location = if !location_parts.is_empty() {
            Some(location_parts.join(" "))
        } else {
            None
        };

        let url = canonical_item_url(&item_id);

        items.push(TaobaoSearchItem {
            item_id,
            title,
            price_cny,
            original_price_cny: None,
            shop_name,
            location,
            sales,
            sales_count,
            url,
        });
    }

    // If zero items were found, verify if visible page text carried challenge wording
    if items.is_empty() {
        let body_sel = Selector::parse("body").unwrap();
        let body_text = document
            .select(&body_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();

        let body_lower = body_text.to_lowercase();
        if body_text.contains("验证")
            || body_text.contains("人机")
            || body_lower.contains("captcha")
            || body_lower.contains("access denied")
            || body_lower.contains("are you human")
        {
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Kasada,
                details: "Taobao anti-bot challenge triggered (zero items with challenge text)"
                    .to_string(),
            });
        }
    }

    Ok(items)
}

/// Parse HTML product card page into `TaobaoCardDetails`.
pub fn parse_card_html(
    html: &str,
    expected_id: Option<&str>,
) -> Result<TaobaoCardDetails, ScraperError> {
    let document = Html::parse_document(html);

    // 1. Check title for gone / 404 indicators
    let title_sel = Selector::parse("title").unwrap();
    let doc_title = document
        .select(&title_sel)
        .next()
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    if doc_title.contains("不存在") || doc_title.contains("很抱歉") || doc_title.contains("已下架")
    {
        return Err(ScraperError::NotFound {
            query: expected_id.unwrap_or("unknown").to_string(),
        });
    }

    // 2. Check login wall or challenge
    check_login_wall_or_challenge(&document)?;

    // 3. Extract title
    let main_title_sel = Selector::parse(
        "[class*='ItemTitle--mainTitle'], [class*='mainTitle'], [class*='MainTitle'], h1",
    )
    .unwrap();
    let mut title = None;
    for elem in document.select(&main_title_sel) {
        if let Some(attr_t) = elem.value().attr("title") {
            let trimmed = attr_t.trim();
            if !trimmed.is_empty() {
                title = Some(trimmed.to_string());
                break;
            }
        }
        let text = elem.text().collect::<String>().trim().to_string();
        if !text.is_empty() {
            title = Some(text);
            break;
        }
    }

    if title.is_none() && !doc_title.is_empty() {
        let clean_title = doc_title
            .split("-淘宝网")
            .next()
            .unwrap_or(&doc_title)
            .split("_淘宝网")
            .next()
            .unwrap_or(&doc_title)
            .trim()
            .to_string();
        if !clean_title.is_empty() {
            title = Some(clean_title);
        }
    }

    // 4. Prices
    // Current / selling price: modern highlightPrice or Price--current
    let current_price_sel = Selector::parse(
        "[class*='Price--current'], [class*='highlightPrice'], [class*='currentPrice']",
    )
    .unwrap();
    let mut price_cny = None;

    if let Some(cur_elem) = document.select(&current_price_sel).next() {
        let text = cur_elem.text().collect::<String>();
        price_cny = parse_cny_price(&text);
    }

    // Strikethrough / original price: Price--original, del, s, subPrice
    let orig_price_sel = Selector::parse(
        "[class*='Price--original'], del, s, [class*='subPrice'], [class*='originalPrice']",
    )
    .unwrap();
    let mut original_price_cny = None;
    if let Some(orig_elem) = document.select(&orig_price_sel).next() {
        let text = orig_elem.text().collect::<String>();
        original_price_cny = parse_cny_price(&text);
    }

    // Fallback if price_cny still None: search inside Price--root or PriceWrap
    if price_cny.is_none() {
        let price_root_sel =
            Selector::parse("[class*='Price--root'], [class*='PriceWrap'], [class*='priceWrap']")
                .unwrap();
        if let Some(root_elem) = document.select(&price_root_sel).next() {
            // Take first price text that is NOT coupon
            let price_text_sel = Selector::parse("[class*='priceText']").unwrap();
            if let Some(p_elem) = root_elem.select(&price_text_sel).next() {
                let text = p_elem.text().collect::<String>();
                price_cny = parse_cny_price(&text);
            }
        }
    }

    // 5. Shop name
    let shop_sel = Selector::parse(
        "[class*='ShopHeader--name'], span[class*='shopName'], a[class*='shopName'], [class*='ShopHeader--root'] a, [class*='shopHeader'] a",
    )
    .unwrap();
    let shop_name = document
        .select(&shop_sel)
        .next()
        .map(|e| e.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty());

    // 6. Sales
    let sales_sel = Selector::parse(
        "[class*='SalesDesc--sales'], [class*='realSales'], [class*='sales'], [class*='Sales']",
    )
    .unwrap();
    let mut sales = None;
    let mut sales_count = None;
    if let Some(sales_elem) = document.select(&sales_sel).next() {
        let s_text = sales_elem.text().collect::<String>().trim().to_string();
        if !s_text.is_empty() {
            sales_count = parse_sales_count(&s_text);
            sales = Some(s_text);
        }
    }

    // 7. Location
    let delivery_sel =
        Selector::parse("[class*='Delivery--root'], [class*='delivery'], [class*='procity']")
            .unwrap();
    let mut location = None;
    if let Some(del_elem) = document.select(&delivery_sel).next() {
        let text = del_elem.text().collect::<String>().trim().to_string();
        if let Some(pos) = text.find('·') {
            let part = text[pos + '·'.len_utf8()..].trim().to_string();
            if !part.is_empty() {
                location = Some(part);
            }
        } else if !text.is_empty() {
            location = Some(text);
        }
    }

    // 8. Description images count
    let desc_img_sel = Selector::parse(
        "[class*='desc-content'] img, [class*='desc'] img, [class*='Desc'] img, #description img",
    )
    .unwrap();
    let description_images = document.select(&desc_img_sel).count();

    // 9. Validation: both title and price cannot be absent
    if title.is_none() && price_cny.is_none() {
        return Err(ScraperError::Parse(
            "Rendered card has neither title nor price".to_string(),
        ));
    }

    let title = title.unwrap_or_default();
    let item_id = expected_id
        .map(ToString::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let url = canonical_item_url(&item_id);

    Ok(TaobaoCardDetails {
        item_id,
        title,
        price_cny,
        original_price_cny,
        shop_name,
        sales,
        sales_count,
        location,
        description_images,
        url,
    })
}
