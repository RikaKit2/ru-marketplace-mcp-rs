use scraper::{Html, Selector};
use std::collections::HashSet;

use ru_marketplace_core::domain::{ScraperError, WafProtection};

use crate::models::{AliExpressCard, AliExpressProduct};

pub const ALIEXPRESS_BASE_URL: &str = "https://aliexpress.ru";
pub const ALIEXPRESS_SEARCH_URL: &str = "https://aliexpress.ru/wholesale";

const JUNK_PRICE_MARKERS: &[&str] = &[
    "доставк",
    "кредит",
    "рассрочк",
    "кэшбэк",
    "кешбэк",
    "бонус",
    "мес",
    "отзыв",
    "балл",
    "пункт",
];

const JUNK_TITLE_MARKERS: &[&str] = &[
    "доставк",
    "бесплатно",
    "дней",
    "рекомендуем",
    "купили",
    "купон",
    "в корзину",
    "купить",
];

/// Check if raw HTML or URL payload indicates an AliExpress x5sec/anti-bot challenge.
pub fn is_punish(html: &str) -> bool {
    let lower = html.to_lowercase();
    lower.contains("punish?x5secdata=")
        || lower.contains("x5secdata")
        || lower.contains("пройдите проверку")
        || lower.contains("sec-firewall")
        || (lower.contains("x5sec") && lower.contains("challenge"))
}

/// Check if response is an unrendered or empty client-side SPA shell without items.
pub fn is_empty_shell(html: &str) -> bool {
    let lower = html.to_lowercase();
    !lower.contains("/item/")
        && (lower.contains("__aer_data__")
            || lower.contains("id=\"root\"")
            || lower.contains("id=\"__next\"")
            || lower.len() < 1200)
}

/// Validate and extract 9-16 digit item ID from bare numeric ID or AliExpress URL.
pub fn extract_item_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // Bare numeric ID: 9 to 16 digits
    if raw.chars().all(|c| c.is_ascii_digit()) && raw.len() >= 9 && raw.len() <= 16 {
        return Some(raw.to_string());
    }

    // Disallow protocol-relative URLs
    if raw.starts_with("//") {
        return None;
    }

    // Scheme & host verification if full URL
    let path = if raw.starts_with("http://") || raw.starts_with("https://") {
        let after_scheme = if let Some(s) = raw.strip_prefix("https://") {
            s
        } else {
            raw.strip_prefix("http://")?
        };
        let slash_idx = after_scheme.find('/')?;
        let host_port = &after_scheme[..slash_idx];
        let host = host_port
            .split(':')
            .next()
            .unwrap_or(host_port)
            .trim_end_matches('.');
        let host_lower = host.to_lowercase();
        if host_lower != "aliexpress.ru" && !host_lower.ends_with(".aliexpress.ru") {
            return None;
        }
        &after_scheme[slash_idx..]
    } else {
        raw
    };

    let item_prefix = "/item/";
    let idx = path.find(item_prefix)?;
    let after_item = &path[idx + item_prefix.len()..];

    let digits: String = after_item
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.len() >= 9 && digits.len() <= 16 {
        Some(digits)
    } else {
        None
    }
}

/// Build canonical card URL from item ID.
pub fn card_url(item_id: &str) -> String {
    format!("{ALIEXPRESS_BASE_URL}/item/{item_id}.html")
}

/// Build catalog search URL for a given query and page.
pub fn search_url(query: &str, page: usize) -> String {
    let trimmed = query.trim();
    let mut encoded = String::with_capacity(trimmed.len() * 3);
    for b in trimmed.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            encoded.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{b:02X}");
        }
    }

    if page <= 1 {
        format!("{ALIEXPRESS_SEARCH_URL}?SearchText={encoded}")
    } else {
        format!("{ALIEXPRESS_SEARCH_URL}?SearchText={encoded}&page={page}")
    }
}

/// Extract price numerical value from text containing currency sign (₽ / руб).
pub fn extract_price_from_currency_string(text: &str) -> Option<f64> {
    let clean = text.replace(['\u{00a0}', ' '], "");
    // Extract first contiguous sequence of digits
    let mut digits = String::new();
    let mut started = false;
    for c in clean.chars() {
        if c.is_ascii_digit() {
            started = true;
            digits.push(c);
        } else if started {
            break;
        }
    }

    if digits.is_empty() {
        None
    } else {
        digits.parse::<f64>().ok().filter(|&v| v > 0.0)
    }
}

/// Extract rating and orders count from card meta description text.
pub fn parse_meta_description(desc: &str) -> (Option<u32>, Option<f32>) {
    let mut orders = None;
    let mut rating = None;

    let desc_clean = desc.replace('\u{00a0}', " ");

    // Orders: "2334 заказов"
    if let Some(idx_ord) = desc_clean.find("заказ") {
        let before = desc_clean[..idx_ord].trim_end();
        let mut digits = Vec::new();
        for c in before.chars().rev() {
            if c.is_ascii_digit() {
                digits.push(c);
            } else if !digits.is_empty() {
                break;
            }
        }
        if !digits.is_empty() {
            digits.reverse();
            let num_str: String = digits.into_iter().collect();
            orders = num_str.parse::<u32>().ok();
        }
    }

    // Rating: "Рейтинг- 4.7" or "Рейтинг: 4.8"
    let desc_lower = desc_clean.to_lowercase();
    if let Some(idx_rat) = desc_lower.find("рейтинг") {
        let after = desc_clean[idx_rat + "рейтинг".len()..]
            .trim_start_matches(|c: char| c == '-' || c == ':' || c == '*' || c.is_whitespace());
        let mut num_str = String::new();
        for c in after.chars() {
            if c.is_ascii_digit() || c == '.' || c == ',' {
                num_str.push(if c == ',' { '.' } else { c });
            } else {
                break;
            }
        }
        if !num_str.is_empty() {
            rating = num_str
                .parse::<f32>()
                .ok()
                .filter(|&r| (0.0..=5.0).contains(&r));
        }
    }

    (orders, rating)
}

/// Parse HTML search grid into a list of AliExpressProduct items.
pub fn parse_search_page(html: &str, limit: usize) -> Result<Vec<AliExpressProduct>, ScraperError> {
    if is_punish(html) {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Kasada,
            details: "AliExpress x5sec challenge triggered during catalog search".to_string(),
        });
    }

    let document = Html::parse_document(html);
    let anchor_selector = match Selector::parse("a[href*='/item/']") {
        Ok(s) => s,
        Err(e) => return Err(ScraperError::Parse(format!("Invalid CSS selector: {e:?}"))),
    };
    let img_selector = match Selector::parse("img") {
        Ok(s) => s,
        Err(e) => return Err(ScraperError::Parse(format!("Invalid img selector: {e:?}"))),
    };

    let mut products = Vec::new();
    let mut seen_ids = HashSet::new();

    for anchor in document.select(&anchor_selector) {
        if products.len() >= limit {
            break;
        }

        let href = anchor.value().attr("href").unwrap_or_default();
        let Some(item_id) = extract_item_id(href) else {
            continue;
        };

        if !seen_ids.insert(item_id.clone()) {
            continue;
        }

        // SKU ID if present
        let sku_id = if let Some(idx) = href.find("sku_id=") {
            let after = &href[idx + "sku_id=".len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                Some(digits)
            }
        } else {
            None
        };

        // Canonical product link
        let full_url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{ALIEXPRESS_BASE_URL}{href}")
        };

        // Product image
        let mut image_url = None;
        for img in anchor.select(&img_selector) {
            let src = img
                .value()
                .attr("src")
                .or_else(|| img.value().attr("data-src"));
            if let Some(s) = src
                && s.starts_with("http")
                && !s.contains("snippet/icon_")
                && !s.contains("snippet/tag_")
            {
                image_url = Some(s.to_string());
                break;
            }
        }

        // Collect string leaves
        let leaf_texts: Vec<String> = anchor
            .text()
            .map(|t| t.replace('\u{00a0}', " ").trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();

        let mut regular_prices = Vec::new();
        let mut coupon_price = None;
        let mut discount_percent = None;
        let mut rating = None;
        let mut orders_count = None;
        let mut best_title: Option<String> = None;
        let mut best_title_len = 0;

        for text in &leaf_texts {
            let text_lower = text.to_lowercase();

            // Discount percent: "-58%" or "58%"
            if text.ends_with('%')
                && (text.starts_with('-')
                    || text.chars().next().is_some_and(|c| c.is_ascii_digit()))
            {
                let clean = text.trim_start_matches('-').trim_end_matches('%').trim();
                if let Ok(pct) = clean.parse::<u32>() {
                    discount_percent = Some(pct);
                    continue;
                }
            }

            // Rating: standalone float "4.7", "5.0"
            if text.len() <= 4 && (text.contains('.') || text.contains(',')) {
                let clean_r = text.replace(',', ".");
                if let Ok(r) = clean_r.parse::<f32>()
                    && (0.0..=5.0).contains(&r)
                {
                    rating = Some(r);
                    continue;
                }
            }

            // Orders count: "2 334 купили"
            if text_lower.contains("купили") {
                let clean_digits: String = text
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == ' ')
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(ord) = clean_digits.parse::<u32>() {
                    orders_count = Some(ord);
                    continue;
                }
            }

            // Prices: containing '₽' or "руб"
            if text.contains('₽') || text_lower.contains("руб") {
                if let Some(val) = extract_price_from_currency_string(text) {
                    if text_lower.contains("купон") {
                        coupon_price = Some(val);
                    } else if !JUNK_PRICE_MARKERS.iter().any(|&j| text_lower.contains(j)) {
                        regular_prices.push(val);
                    }
                }
                continue;
            }

            // Title candidate
            if JUNK_TITLE_MARKERS.iter().any(|&j| text_lower.contains(j)) {
                continue;
            }

            // Avoid predominantly numeric leaves
            let digit_count = text.chars().filter(|c| c.is_ascii_digit()).count();
            if digit_count >= 2 && (digit_count as f64 / text.len() as f64) > 0.4 {
                continue;
            }

            if text.len() > best_title_len {
                best_title_len = text.len();
                best_title = Some(text.clone());
            }
        }

        // Smallest regular price is current customer price
        let price_rub = regular_prices.iter().copied().reduce(f64::min);
        // Strikethrough / old price is highest candidate strictly above price_rub
        let old_price_rub = if let Some(current) = price_rub {
            regular_prices
                .iter()
                .copied()
                .filter(|&p| p > current)
                .reduce(f64::max)
        } else {
            None
        };

        let title = best_title.unwrap_or_else(|| format!("AliExpress item {item_id}"));

        products.push(AliExpressProduct {
            item_id,
            title,
            price_rub,
            old_price_rub,
            coupon_price_rub: coupon_price,
            discount_percent,
            rating,
            orders_count,
            sku_id,
            url: full_url,
            image_url,
            seller: Some("AliExpress".to_string()),
        });
    }

    Ok(products)
}

/// Parse HTML product card page into AliExpressCard details.
pub fn parse_card_page(html: &str, item_id: &str) -> Result<AliExpressCard, ScraperError> {
    if is_punish(html) {
        return Err(ScraperError::WafBlocked {
            waf: WafProtection::Kasada,
            details: "AliExpress x5sec challenge triggered on product card".to_string(),
        });
    }

    let document = Html::parse_document(html);

    // Title from h1, meta or title tag
    let h1_selector = Selector::parse("h1").unwrap();
    let meta_title_selector =
        Selector::parse("meta[property='og:title'], meta[name='title']").unwrap();
    let title_selector = Selector::parse("title").unwrap();

    let mut title = None;
    if let Some(h1_elem) = document.select(&h1_selector).next() {
        let t = h1_elem
            .text()
            .collect::<String>()
            .replace('\u{00a0}', " ")
            .trim()
            .to_string();
        if !t.is_empty() {
            title = Some(t);
        }
    }

    if title.is_none() {
        for meta in document.select(&meta_title_selector) {
            if let Some(content) = meta.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.to_string());
                    break;
                }
            }
        }
    }

    if title.is_none()
        && let Some(title_elem) = document.select(&title_selector).next()
    {
        let t = title_elem.text().collect::<String>();
        let clean = t
            .split('|')
            .next()
            .unwrap_or(&t)
            .split('-')
            .next()
            .unwrap_or(&t)
            .trim()
            .to_string();
        if !clean.is_empty() {
            title = Some(clean);
        }
    }

    let title = title.unwrap_or_else(|| format!("AliExpress item {item_id}"));

    // Meta description for orders and rating
    let meta_desc_selector =
        Selector::parse("meta[name='description'], meta[property='og:description']").unwrap();
    let mut description = None;
    let mut orders_count = None;
    let mut rating = None;

    for meta in document.select(&meta_desc_selector) {
        if let Some(content) = meta.value().attr("content") {
            let desc_str = content.trim();
            if !desc_str.is_empty() {
                description = Some(desc_str.to_string());
                let (ord, rat) = parse_meta_description(desc_str);
                if orders_count.is_none() {
                    orders_count = ord;
                }
                if rating.is_none() {
                    rating = rat;
                }
            }
        }
    }

    // Image URL
    let meta_img_selector = Selector::parse("meta[property='og:image']").unwrap();
    let img_selector = Selector::parse("img").unwrap();
    let mut image_url = None;

    if let Some(meta_img) = document.select(&meta_img_selector).next()
        && let Some(content) = meta_img.value().attr("content")
        && content.starts_with("http")
    {
        image_url = Some(content.to_string());
    }

    if image_url.is_none() {
        for img in document.select(&img_selector) {
            let src = img
                .value()
                .attr("src")
                .or_else(|| img.value().attr("data-src"));
            if let Some(s) = src
                && s.starts_with("http")
                && (s.contains("ae-pic") || s.contains("alicdn"))
            {
                image_url = Some(s.to_string());
                break;
            }
        }
    }

    // Prices from sticky offer or DOM
    let price_selector = Selector::parse(
        "[class*='HazeStickyOfferPrice'], [class*='SnowStickyOffer'], [class*='offerPrice'], [class*='price']",
    ).unwrap();

    let mut regular_prices = Vec::new();
    for price_elem in document.select(&price_selector) {
        let text = price_elem
            .text()
            .collect::<String>()
            .replace('\u{00a0}', " ");
        let text_lower = text.to_lowercase();
        if (text.contains('₽') || text_lower.contains("руб"))
            && !JUNK_PRICE_MARKERS.iter().any(|&j| text_lower.contains(j))
            && let Some(val) = extract_price_from_currency_string(&text)
        {
            regular_prices.push(val);
        }
    }

    let price_rub = regular_prices.iter().copied().reduce(f64::min);
    let old_price_rub = if let Some(current) = price_rub {
        regular_prices
            .iter()
            .copied()
            .filter(|&p| p > current)
            .reduce(f64::max)
    } else {
        None
    };

    let html_lower = html.to_lowercase();
    let in_stock =
        !html_lower.contains("товар закончился") && !html_lower.contains("нет в наличии");

    Ok(AliExpressCard {
        item_id: item_id.to_string(),
        title,
        price_rub,
        old_price_rub,
        rating,
        orders_count,
        url: card_url(item_id),
        image_url,
        description,
        seller: Some("AliExpress".to_string()),
        in_stock,
    })
}
