# Fix Parsers (DNS, Citilink, Ozon) and Wildberries CDP Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix product parsing failures for DNS-Shop and Citilink by transitioning to modern HTML DOM scraping via `scraper`, enhance Ozon Composer API widget handling for modern catalog layouts, and resolve Wildberries `TypeError: Failed to fetch` by stabilizing CDP tab origins and injecting genuine browser request headers.

**Architecture:** 
- `crates/dns`: Introduce `scraper` dependency; parse modern `.catalog-product` DOM structure (titles, prices, links, IDs, ratings) while maintaining JSON fallback.
- `crates/citilink`: Introduce `scraper` dependency; parse `<script id="__NEXT_DATA__">` and modern `[data-meta-name="ProductCard"]` / `[data-meta-name="SnippetProduct"]` DOM selectors while maintaining JSON fallback.
- `crates/ozon`: Expand search widget prefixes to include `searchResultsV3-`, `searchMergedDesktop-`, support nested `megaPaginator` items, handle `priceV3` / `splitPrice`, and relax title string length restrictions.
- `crates/solver` & `crates/wb`: Ensure tab origin stabilization (`window.location.origin` and `document.readyState`) before executing `fetch()` via CDP, inject `x-requested-with: XMLHttpRequest` and standard browser headers into the JS fetch expression, preventing CORS and WAF fetch rejections.

**Tech Stack:** Rust 2024 edition, Cargo workspace, `scraper = "0.23"`, `serde_json`, `tokio`, Chrome DevTools Protocol (`NativeCdpSolver`).

**Spec:** User test report v0.2.2:
1. `dns_search`: Qrator bypassed, but 0 offers due to obsolete `data-product=""` substring search instead of `.catalog-product` DOM.
2. `citilink_search`: Challenge bypassed, but 0 offers due to obsolete `data-params=''` substring search instead of `[data-meta-name="ProductCard"]` DOM.
3. `ozon_search`: Adapt Composer API widget mapping (`searchResultsV3`, `megaPaginator`, flexible title heuristics).
4. `wb_search`: Fix `TypeError: Failed to fetch` by ensuring tab origin readiness and providing browser headers (`x-requested-with`, `Accept: */*`).

## Global Constraints

- Must compile cleanly with Rust 2024 edition and Cranelift/native target configuration (`.cargo/config.toml`).
- All Cargo compilation and test commands MUST use `-j 2` due to host CPU limits (2 cores / 4 threads).
- Standard web security must remain intact: do NOT use `--disable-web-security`. Maintain isolated tabs with proper origin domains.
- Stdio transport integrity: all logs strictly to `stderr`, `stdout` strictly reserved for JSON-RPC 2.0 messages.
- Git commits: NEVER commit without explicit user instruction. When instructed, MUST use cryptographic signing (`-S`) and English Conventional Commits (`feat: ...`, `fix: ...`).
- Pre-commit checks (`cargo fmt --check`, `cargo clippy -j 2 --all-targets -- -D warnings`, `cargo test -j 2`) must pass cleanly.
- Release binary: update `dist/ru-marketplace-mcp` on completion for Nix flake packaging compatibility.

---

### Task 1: DNS-Shop Modern HTML DOM Parser (`crates/dns`)

**Files:**
- Modify: `Cargo.toml` (root workspace dependencies)
- Modify: `crates/dns/Cargo.toml`
- Modify: `crates/dns/src/connector.rs`
- Test: `crates/dns/tests/dns_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core::domain::{ProductOffer, PriceTier, Marketplace, Currency, StockStatus, ScraperError}`
- Produces: `DnsConnector::parse_catalog_data(content: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError>` supporting modern `.catalog-product` HTML layout.

- [ ] **Step 1: Add `scraper` to workspace and `crates/dns/Cargo.toml`**

In root `Cargo.toml`:
```toml
[workspace.dependencies]
scraper = "0.23.1"
```

In `crates/dns/Cargo.toml`:
```toml
[dependencies]
scraper.workspace = true
```

- [ ] **Step 2: Write failing unit test for modern DNS HTML catalog in `crates/dns/tests/dns_tests.rs`**

Add the test to `crates/dns/tests/dns_tests.rs`:
```rust
#[test]
fn test_dns_parse_modern_html_catalog() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <div class="products-list">
            <div class="catalog-product ui-button-widget" data-product="1029384" data-code="1029384">
                <a class="catalog-product__name ui-link ui-link_black" href="/product/1029384/processor-intel-core-i5-14600k/">
                    <span>Процессор Intel Core i5-14600K OEM</span>
                </a>
                <div class="catalog-product__price">
                    <div class="product-buy__price product-buy__price_active">27 999&nbsp;₽</div>
                </div>
                <div class="catalog-product__rating" data-rating="4.8">4.8 (64)</div>
            </div>
            <div class="catalog-product ui-button-widget" data-product="1029385" data-code="1029385">
                <a class="catalog-product__name ui-link ui-link_black" href="/product/1029385/processor-amd-ryzen-7-7800x3d/">
                    <span>Процессор AMD Ryzen 7 7800X3D OEM</span>
                </a>
                <div class="catalog-product__price">
                    <div class="product-buy__price product-buy__price_active">41 499 ₽</div>
                </div>
            </div>
        </div>
    </body>
    </html>
    "#;

    let offers = DnsConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 2);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::Dns);
    assert_eq!(first.title, "Процессор Intel Core i5-14600K OEM");
    assert_eq!(first.price_tier, PriceTier::Base(27999));
    assert_eq!(first.url, "https://www.dns-shop.ru/product/1029384/processor-intel-core-i5-14600k/");
    assert_eq!(first.product_id, Some("1029384".to_string()));
    assert_eq!(first.seller, Some("DNS".to_string()));
    assert_eq!(first.rating, Some(4.8));

    let second = &offers[1];
    assert_eq!(second.title, "Процессор AMD Ryzen 7 7800X3D OEM");
    assert_eq!(second.price_tier, PriceTier::Base(41499));
    assert_eq!(second.product_id, Some("1029385".to_string()));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ru-marketplace-dns --test dns_tests test_dns_parse_modern_html_catalog -j 2`
Expected: FAIL (returns 0 offers because `data-product="` substring search expects JSON inside quotes).

- [ ] **Step 4: Implement modern DOM parsing in `crates/dns/src/connector.rs`**

Update `parse_catalog_data` in `crates/dns/src/connector.rs`:
```rust
use scraper::{Html, Selector};

pub fn parse_catalog_data(
    content: &str,
    limit: usize,
) -> Result<Vec<ProductOffer>, ScraperError> {
    let trimmed = content.trim();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        return Self::parse_json_products(&val, limit);
    }

    let mut offers = Vec::new();

    // 1. Try script tag with JSON product snippets
    if let Some(script_data) = extract_between(
        content,
        "<script data-product-snippets type=\"application/json\">",
        "</script>",
    ) && let Ok(val) = serde_json::from_str::<Value>(&script_data)
        && let Ok(parsed) = Self::parse_json_products(&val, limit)
    {
        offers.extend(parsed);
    }

    if !offers.is_empty() {
        return Ok(offers);
    }

    // 2. Try modern HTML DOM parsing via scraper
    let document = Html::parse_document(content);
    let card_selector = Selector::parse(".catalog-product, .catalog-item, [data-product-id]").unwrap();
    let title_selector = Selector::parse(".catalog-product__name, a[data-role='product-name'], .catalog-product__title, a.ui-link").unwrap();
    let price_selector = Selector::parse(".product-buy__price_active, .product-buy__price, .catalog-product__price").unwrap();
    let rating_selector = Selector::parse(".catalog-product__rating, [data-rating]").unwrap();

    for element in document.select(&card_selector) {
        if offers.len() >= limit {
            break;
        }

        // Title and URL
        let title_elem = element.select(&title_selector).next();
        let title = title_elem
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let link = title_elem
            .and_then(|e| e.value().attr("href"))
            .unwrap_or_default();
        let full_url = if link.starts_with("http") {
            link.to_string()
        } else if !link.is_empty() {
            format!("https://www.dns-shop.ru{}", link)
        } else {
            String::new()
        };

        // Price
        let price_text = element
            .select(&price_selector)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();
        let price = match Self::parse_price_str(&price_text) {
            Some(p) => p,
            None => continue,
        };

        // Product ID / Code
        let product_id = element
            .value()
            .attr("data-product")
            .or_else(|| element.value().attr("data-code"))
            .or_else(|| element.value().attr("data-product-id"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                // Try extracting id from url: /product/(\d+)/
                link.split("/product/")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .map(|id| id.to_string())
            });

        // Rating
        let rating = element
            .select(&rating_selector)
            .next()
            .and_then(|e| {
                e.value().attr("data-rating").and_then(|r| r.parse::<f32>().ok()).or_else(|| {
                    let text = e.text().collect::<String>();
                    text.split_whitespace().next().and_then(|s| s.parse::<f32>().ok())
                })
            });

        offers.push(ProductOffer {
            marketplace: Marketplace::Dns,
            title,
            url: full_url,
            price_tier: PriceTier::Base(price),
            original_price: None,
            currency: Currency::Rub,
            rating,
            review_count: None,
            stock: StockStatus::InStock(1),
            seller: Some("DNS".to_string()),
            product_id,
        });
    }

    if !offers.is_empty() {
        return Ok(offers);
    }

    // 3. Fallback to legacy data-product JSON substring parsing
    let mut search_pos = 0;
    while let Some(start) = content[search_pos..].find("data-product=\"") {
        if offers.len() >= limit {
            break;
        }
        let abs_start = search_pos + start + "data-product=\"".len();
        if let Some(end) = content[abs_start..].find('"') {
            let raw_json = &content[abs_start..abs_start + end];
            let unescaped = raw_json
                .replace("&quot;", "\"")
                .replace("&amp;", "&")
                .replace("&#39;", "'");

            if let Ok(val) = serde_json::from_str::<Value>(&unescaped) {
                let name = val.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let price = val.get("price").and_then(|v| v.as_u64()).unwrap_or(0);
                let id = val
                    .get("id")
                    .or_else(|| val.get("sku"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if !name.is_empty() && price > 0 {
                    offers.push(ProductOffer {
                        marketplace: Marketplace::Dns,
                        title: name.to_string(),
                        url: format!("https://www.dns-shop.ru/product/{id}/"),
                        price_tier: PriceTier::Base(price),
                        original_price: None,
                        currency: Currency::Rub,
                        rating: None,
                        review_count: None,
                        stock: StockStatus::InStock(1),
                        seller: Some("DNS".to_string()),
                        product_id: Some(id.to_string()),
                    });
                }
            }
            search_pos = abs_start + end;
        } else {
            break;
        }
    }

    Ok(offers)
}
```

- [ ] **Step 5: Run tests to verify all DNS tests pass**

Run: `cargo test -p ru-marketplace-dns -j 2`
Expected: PASS (both `test_dns_parse_catalog_data` and `test_dns_parse_modern_html_catalog` pass).

---

### Task 2: Ситилинк Modern HTML DOM and Next.js Parser (`crates/citilink`)

**Files:**
- Modify: `crates/citilink/Cargo.toml`
- Modify: `crates/citilink/src/connector.rs`
- Test: `crates/citilink/tests/citilink_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core::domain::{ProductOffer, PriceTier, Marketplace, Currency, StockStatus, ScraperError}`
- Produces: `CitilinkConnector::parse_catalog_data(content: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError>` supporting modern `[data-meta-name="ProductCard"]` and `<script id="__NEXT_DATA__">`.

- [ ] **Step 1: Add `scraper` to `crates/citilink/Cargo.toml`**

In `crates/citilink/Cargo.toml`:
```toml
[dependencies]
scraper.workspace = true
```

- [ ] **Step 2: Write failing unit test for modern Citilink HTML catalog in `crates/citilink/tests/citilink_tests.rs`**

Add tests to `crates/citilink/tests/citilink_tests.rs`:
```rust
#[test]
fn test_citilink_parse_modern_html_catalog() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <div class="product-cards-layout">
            <div class="app-catalog-1tp040a" data-meta-name="ProductCard" data-meta-product-id="1948271">
                <a class="app-catalog-9ff07x" data-meta-name="ProductCard__title" title="Процессор Intel Core i5 14600K LGA 1700 OEM" href="/product/processor-intel-core-i5-14600k-lga-1700-oem-1948271/">
                    Процессор Intel Core i5 14600K LGA 1700 OEM
                </a>
                <div data-meta-name="ProductCard__price">
                    <span class="app-catalog-price" data-meta-price="28700">28 700 ₽</span>
                </div>
                <div data-meta-name="ProductCard__rating" data-meta-rating="4.9">4.9</div>
                <div data-meta-name="ProductCard__reviews">28 отзывов</div>
            </div>
            <div class="app-catalog-1tp040a" data-meta-name="SnippetProduct" data-meta-product-id="1948272">
                <a data-meta-name="SnippetProduct__title" title="Процессор AMD Ryzen 7 7800X3D AM5 OEM" href="/product/processor-amd-ryzen-7-7800x3d-am5-oem-1948272/">
                    Процессор AMD Ryzen 7 7800X3D AM5 OEM
                </a>
                <div data-meta-name="SnippetProduct__price">
                    <span>42 990&nbsp;₽</span>
                </div>
            </div>
        </div>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 2);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::Citilink);
    assert_eq!(first.title, "Процессор Intel Core i5 14600K LGA 1700 OEM");
    assert_eq!(first.price_tier, PriceTier::Base(28700));
    assert_eq!(first.url, "https://www.citilink.ru/product/processor-intel-core-i5-14600k-lga-1700-oem-1948271/");
    assert_eq!(first.product_id, Some("1948271".to_string()));
    assert_eq!(first.seller, Some("Citilink".to_string()));
    assert_eq!(first.rating, Some(4.9));
    assert_eq!(first.review_count, Some(28));

    let second = &offers[1];
    assert_eq!(second.title, "Процессор AMD Ryzen 7 7800X3D AM5 OEM");
    assert_eq!(second.price_tier, PriceTier::Base(42990));
    assert_eq!(second.product_id, Some("1948272".to_string()));
}

#[test]
fn test_citilink_parse_next_data_script() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <script id="__NEXT_DATA__" type="application/json">
        {
            "props": {
                "pageProps": {
                    "initialState": {
                        "products": [
                            {
                                "id": "1892831",
                                "name": "Кулер для процессора Deepcool AK620",
                                "price": 5490,
                                "url": "/product/kuler-deepcool-ak620-1892831/"
                            }
                        ]
                    }
                }
            }
        }
        </script>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Кулер для процессора Deepcool AK620");
    assert_eq!(offers[0].price_tier, PriceTier::Base(5490));
    assert_eq!(offers[0].product_id, Some("1892831".to_string()));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ru-marketplace-citilink --test citilink_tests test_citilink_parse_modern_html_catalog -j 2`
Expected: FAIL (returns 0 offers because `data-params='` substring search is missing).

- [ ] **Step 4: Implement modern DOM and `__NEXT_DATA__` parsing in `crates/citilink/src/connector.rs`**

Update `parse_catalog_data` in `crates/citilink/src/connector.rs`:
```rust
use scraper::{Html, Selector};

pub fn parse_catalog_data(
    content: &str,
    limit: usize,
) -> Result<Vec<ProductOffer>, ScraperError> {
    let trimmed = content.trim();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        return Self::parse_json_products(&val, limit);
    }

    let mut offers = Vec::new();

    // 1. Check window.__INITIAL_STATE__
    if let Some(script_data) =
        extract_between(content, "window.__INITIAL_STATE__ = ", ";</script>")
        && let Ok(val) = serde_json::from_str::<Value>(&script_data)
        && let Some(products) = val
            .get("catalog")
            .and_then(|c| c.get("products"))
            .or_else(|| val.get("products"))
        && let Ok(parsed) = Self::parse_json_products(products, limit)
    {
        offers.extend(parsed);
    }

    // 2. Check <script id="__NEXT_DATA__">
    if offers.is_empty()
        && let Some(next_data) = extract_between(
            content,
            "<script id=\"__NEXT_DATA__\" type=\"application/json\">",
            "</script>",
        )
        && let Ok(val) = serde_json::from_str::<Value>(&next_data)
    {
        if let Some(products) = val
            .pointer("/props/pageProps/initialState/products")
            .or_else(|| val.pointer("/props/pageProps/products"))
            .or_else(|| val.pointer("/props/pageProps/catalog/products"))
        {
            if let Ok(parsed) = Self::parse_json_products(products, limit) {
                offers.extend(parsed);
            }
        }
    }

    if !offers.is_empty() {
        return Ok(offers);
    }

    // 3. Modern HTML DOM parsing via scraper
    let document = Html::parse_document(content);
    let card_selector = Selector::parse(
        "[data-meta-name='ProductCard'], [data-meta-name='SnippetProduct'], [data-meta-name='ProductCardShort'], div[data-product-id]",
    )
    .unwrap();
    let title_selector = Selector::parse(
        "a[data-meta-name$='__title'], [data-meta-name='ProductCardTitle'], a.app-catalog-link, [title]",
    )
    .unwrap();
    let price_selector = Selector::parse(
        "[data-meta-name$='__price'], [data-meta-name='ProductPrice'], span[data-meta-price], .app-catalog-price",
    )
    .unwrap();
    let rating_selector = Selector::parse(
        "[data-meta-name$='__rating'], [data-meta-name='Rating'], [data-meta-rating]",
    )
    .unwrap();
    let reviews_selector = Selector::parse(
        "[data-meta-name$='__reviews'], [data-meta-name='Reviews']",
    )
    .unwrap();

    for element in document.select(&card_selector) {
        if offers.len() >= limit {
            break;
        }

        // Title and URL
        let title_elem = element.select(&title_selector).next();
        let title = title_elem
            .and_then(|e| {
                e.value().attr("title").map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
            })
            .or_else(|| {
                title_elem.map(|e| e.text().collect::<String>().trim().to_string()).filter(|s| !s.is_empty())
            })
            .unwrap_or_default();

        if title.is_empty() {
            continue;
        }

        let link = title_elem
            .and_then(|e| e.value().attr("href"))
            .unwrap_or_default();
        let full_url = if link.starts_with("http") {
            link.to_string()
        } else if !link.is_empty() {
            format!("https://www.citilink.ru{}", link)
        } else {
            String::new()
        };

        // Price
        let price_elem = element.select(&price_selector).next();
        let price = price_elem
            .and_then(|e| e.value().attr("data-meta-price").and_then(|p| p.parse::<u64>().ok()))
            .or_else(|| {
                let text = price_elem.map(|e| e.text().collect::<String>()).unwrap_or_default();
                Self::parse_price_str(&text)
            });

        let price = match price {
            Some(p) if p > 0 => p,
            _ => continue,
        };

        // Product ID
        let product_id = element
            .value()
            .attr("data-meta-product-id")
            .or_else(|| element.value().attr("data-product-id"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                // Try extracting id from URL: /product/...-(\d+)/
                link.trim_end_matches('/')
                    .split('-')
                    .last()
                    .and_then(|last| last.chars().all(|c| c.is_ascii_digit()).then(|| last.to_string()))
            });

        // Rating
        let rating = element.select(&rating_selector).next().and_then(|e| {
            e.value()
                .attr("data-meta-rating")
                .and_then(|r| r.parse::<f32>().ok())
                .or_else(|| {
                    let text = e.text().collect::<String>();
                    text.split_whitespace().next().and_then(|s| s.parse::<f32>().ok())
                })
        });

        // Review count
        let review_count = element.select(&reviews_selector).next().and_then(|e| {
            let text = e.text().collect::<String>();
            let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().ok()
        });

        offers.push(ProductOffer {
            marketplace: Marketplace::Citilink,
            title,
            url: full_url,
            price_tier: PriceTier::Base(price),
            original_price: None,
            currency: Currency::Rub,
            rating,
            review_count,
            stock: StockStatus::InStock(1),
            seller: Some("Citilink".to_string()),
            product_id,
        });
    }

    if !offers.is_empty() {
        return Ok(offers);
    }

    // 4. Fallback to legacy data-params substring parsing
    let mut search_pos = 0;
    while let Some(start) = content[search_pos..].find("data-params='") {
        if offers.len() >= limit {
            break;
        }
        let abs_start = search_pos + start + "data-params='".len();
        if let Some(end) = content[abs_start..].find('\'') {
            let raw_json = &content[abs_start..abs_start + end];
            if let Ok(val) = serde_json::from_str::<Value>(raw_json) {
                let name = val
                    .get("shortName")
                    .or_else(|| val.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let price = val.get("price").and_then(|v| v.as_u64()).unwrap_or(0);
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or_default();

                if !name.is_empty() && price > 0 {
                    offers.push(ProductOffer {
                        marketplace: Marketplace::Citilink,
                        title: name.to_string(),
                        url: format!("https://www.citilink.ru/product/{id}/"),
                        price_tier: PriceTier::Base(price),
                        original_price: None,
                        currency: Currency::Rub,
                        rating: None,
                        review_count: None,
                        stock: StockStatus::InStock(1),
                        seller: Some("Citilink".to_string()),
                        product_id: Some(id.to_string()),
                    });
                }
            }
            search_pos = abs_start + end;
        } else {
            break;
        }
    }

    Ok(offers)
}
```

- [ ] **Step 5: Run tests to verify all Citilink tests pass**

Run: `cargo test -p ru-marketplace-citilink -j 2`
Expected: PASS (existing `test_citilink_parse_catalog_data`, `test_citilink_parse_modern_html_catalog`, `test_citilink_parse_next_data_script`).

---

### Task 3: Ozon Composer API Widget Extensions and Adaptive Tile Parser (`crates/ozon`)

**Files:**
- Modify: `crates/ozon/src/parser.rs`
- Test: `crates/ozon/tests/ozon_tests.rs`

**Interfaces:**
- Consumes: Ozon Composer API JSON (`widgetStates`)
- Produces: `parse_composer_json(json_str: &str, limit: usize) -> Result<Vec<ProductOffer>, ScraperError>` with support for `searchResultsV3-`, `searchMergedDesktop-`, nested `megaPaginator`, `priceV3` / `splitPrice` atoms, and short titles.

- [ ] **Step 1: Write failing unit tests in `crates/ozon/tests/ozon_tests.rs`**

Add tests to `crates/ozon/tests/ozon_tests.rs`:
```rust
#[test]
fn test_ozon_parse_composer_v3_and_merged_widgets() {
    let mock_json = r#"{
        "widgetStates": {
            "searchResultsV3-39281-default-1": "{\"items\": [{\"sku\": \"3087570418\", \"action\": {\"link\": \"/product/3087570418/\"}, \"mainState\": [{\"type\": \"textAtom\", \"textAtom\": {\"text\": \"i5-14600K\"}}, {\"type\": \"priceV3\", \"priceV3\": {\"price\": [{\"text\": \"27 719 ₽\", \"theme\": \"ozon_card\"}]}}]}]}"
        }
    }"#;

    let offers = parse_composer_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 1);
    let offer = &offers[0];
    assert_eq!(offer.title, "i5-14600K");
    assert_eq!(offer.product_id, Some("3087570418".to_string()));
    assert_eq!(offer.price_tier, PriceTier::MarketplaceCard(27719));
}

#[test]
fn test_ozon_parse_composer_mega_paginator_nested_products() {
    let mock_json = r#"{
        "widgetStates": {
            "megaPaginator-12345-default-1": "{\"products\": [{\"id\": 987654321, \"action\": {\"link\": \"/product/987654321/\"}, \"mainState\": [{\"type\": \"textAtom\", \"textAtom\": {\"text\": \"Ryzen 7 7800X3D\"}}, {\"type\": \"price\", \"price\": {\"price\": [{\"text\": \"41 490 ₽\"}]}}]}]}"
        }
    }"#;

    let offers = parse_composer_json(mock_json, 10).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Ryzen 7 7800X3D");
    assert_eq!(offers[0].price_tier, PriceTier::Base(41490));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ru-marketplace-ozon --test ozon_tests test_ozon_parse_composer_v3_and_merged_widgets -j 2`
Expected: FAIL (unrecognized `searchResultsV3-` widget prefix or title rejected due to `text.len() <= 10`).

- [ ] **Step 3: Update `crates/ozon/src/parser.rs`**

In `crates/ozon/src/parser.rs`:
1. Expand widget recognition and item extraction:
```rust
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
        let items: Vec<&Value> = if let Some(arr) = widget_obj.get("items").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else if let Some(arr) = widget_obj.get("products").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else if let Some(arr) = widget_obj.get("elements").and_then(|v| v.as_array()) {
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
```

2. Relax title validation and support `priceV3` / `splitPrice` in `parse_search_tile`:
```rust
        match atom_type {
            "textAtom" => {
                let atom_obj = atom.get("textAtom").unwrap_or(atom);
                let text = atom_obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .trim();

                // Relaxed title extraction: do not reject short titles like "i5-14600K" or "SSD 1TB"
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
                        let text = pi.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                        let theme = pi.get("theme").and_then(|v| v.as_str()).unwrap_or_default();

                        if let Some(p) = parse_price_str(text) {
                            if price == 0 {
                                price = p;
                                if theme.contains("card")
                                    || theme.contains("green")
                                    || theme.contains("ozon_card")
                                {
                                    has_card_price = true;
                                }
                            } else if price_original.is_none() && p > price {
                                price_original = Some(p);
                            }
                        }
                    }
                } else if let Some(text) = atom_obj.get("price").and_then(|v| v.as_str()) {
                    if let Some(p) = parse_price_str(text) {
                        price = p;
                    }
                }
            }
            // other atom types remain unchanged...
```

- [ ] **Step 4: Run tests to verify all Ozon tests pass**

Run: `cargo test -p ru-marketplace-ozon -j 2`
Expected: PASS (all tests in `ozon_tests.rs` pass).

---

### Task 4: Native CDP Solver Origin Stabilization and Wildberries Browser Headers (`crates/solver` & `crates/wb`)

**Files:**
- Modify: `crates/solver/src/native_solver.rs`
- Modify: `crates/wb/src/connector.rs`
- Test: `crates/solver/tests/solver_tests.rs`
- Test: `crates/wb/tests/wb_tests.rs`

**Interfaces:**
- Consumes: `NativeCdpSolver::fetch_in_tab(marketplace: Marketplace, fetch_url: &str) -> Result<String, ScraperError>`
- Produces: Browser-compliant `fetch()` invocation guaranteed to execute after tab origin stabilization, with `x-requested-with: XMLHttpRequest` headers preventing `TypeError: Failed to fetch`.

- [ ] **Step 1: Write unit test in `crates/solver/tests/solver_tests.rs` validating fetch script format**

Check that the JS fetch script construction includes the required browser headers:
```rust
#[test]
fn test_fetch_expression_headers() {
    let url = "https://www.wildberries.ru/__internal/u-search/exactmatch/ru/common/v18/search?query=test";
    let encoded_url = serde_json::to_string(url).unwrap();
    let expr = format!(
        r#"fetch({}, {{ method: "GET", headers: {{ "Accept": "*/*", "Accept-Language": "ru-RU,ru;q=0.9", "x-requested-with": "XMLHttpRequest" }}, credentials: "include" }}).then(async r => ({{ status: r.status, ok: r.ok, text: await r.text() }})).catch(err => ({{ status: 0, ok: false, text: String(err) }}))"#,
        encoded_url
    );

    assert!(expr.contains("x-requested-with"));
    assert!(expr.contains("XMLHttpRequest"));
    assert!(expr.contains("Accept-Language"));
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p ru-marketplace-solver --test solver_tests test_fetch_expression_headers -j 2`
Expected: PASS.

- [ ] **Step 3: Update `crates/solver/src/native_solver.rs` with origin stabilization and headers**

1. In `crates/solver/src/native_solver.rs`, update `start_session`:
Ensure the Wildberries tab has enough time to settle its initial origin:
```rust
        // 2. Open Wildberries tab
        info!("Opening Wildberries tab for authentic TLS Client Hello...");
        let wb_session_id = cdp
            .create_and_attach_tab("https://www.wildberries.ru/")
            .await?;
        tabs.insert(Marketplace::Wildberries, wb_session_id.clone());
        // Wait briefly for Wildberries DOM & origin to settle
        tokio::time::sleep(Duration::from_millis(1500)).await;
```

2. In `crates/solver/src/native_solver.rs`, update `fetch_in_tab`:
```rust
    pub async fn fetch_in_tab(
        &self,
        marketplace: Marketplace,
        fetch_url: &str,
    ) -> Result<String, ScraperError> {
        let encoded_url =
            serde_json::to_string(fetch_url).unwrap_or_else(|_| format!("\"{}\"", fetch_url));

        let expr = format!(
            r#"fetch({}, {{ method: "GET", headers: {{ "Accept": "*/*", "Accept-Language": "ru-RU,ru;q=0.9", "x-requested-with": "XMLHttpRequest" }}, credentials: "include" }}).then(async r => ({{ status: r.status, ok: r.ok, text: await r.text() }})).catch(err => ({{ status: 0, ok: false, text: String(err) }}))"#,
            encoded_url
        );

        let target_origin = match marketplace {
            Marketplace::Ozon => "https://www.ozon.ru",
            Marketplace::Wildberries => "https://www.wildberries.ru",
            Marketplace::Dns => "https://www.dns-shop.ru",
            Marketplace::Citilink => "https://www.citilink.ru",
            _ => "",
        };

        // Helper wait script to ensure tab origin is matched and document is ready
        let check_ready_expr = format!(
            r#"(() => {{
                if (window.location.origin !== "{}") return false;
                if (document.readyState === "loading") return false;
                return true;
            }})()"#,
            target_origin
        );

        // 1. Try persistent session
        {
            let mut session_guard = self.persistent_session.lock().await;
            if let Some(session) = session_guard.as_mut() {
                session.last_used = std::time::Instant::now();

                let tab_session_id = match session.tabs.get(&marketplace) {
                    Some(id) => id.clone(),
                    None => {
                        let target_url = match marketplace {
                            Marketplace::Ozon => "https://www.ozon.ru/",
                            Marketplace::Wildberries => "https://www.wildberries.ru/",
                            Marketplace::Dns => "https://www.dns-shop.ru/",
                            Marketplace::Citilink => "https://www.citilink.ru/",
                            _ => "about:blank",
                        };
                        let new_id = session.cdp.create_and_attach_tab(target_url).await?;
                        session.tabs.insert(marketplace, new_id.clone());
                        new_id
                    }
                };

                // Poll origin readiness up to 3 seconds if target_origin is non-empty
                if !target_origin.is_empty() {
                    let start_wait = std::time::Instant::now();
                    while start_wait.elapsed() < Duration::from_millis(3000) {
                        let ready_val = session
                            .cdp
                            .send_target_cmd(
                                &tab_session_id,
                                "Runtime.evaluate",
                                json!({ "expression": check_ready_expr, "returnByValue": true }),
                            )
                            .await;
                        if let Ok(val) = ready_val
                            && val["result"]["result"]["value"].as_bool() == Some(true)
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }

                let res = session
                    .cdp
                    .send_target_cmd(
                        &tab_session_id,
                        "Runtime.evaluate",
                        json!({
                            "expression": expr,
                            "awaitPromise": true,
                            "returnByValue": true
                        }),
                    )
                    .await;

                if let Ok(val) = res {
                    let status = val["result"]["result"]["value"]["status"]
                        .as_u64()
                        .unwrap_or(0);
                    let ok = val["result"]["result"]["value"]["ok"]
                        .as_bool()
                        .unwrap_or(false);
                    let text = val["result"]["result"]["value"]["text"]
                        .as_str()
                        .unwrap_or_default();

                    if ok && status == 200 {
                        return Ok(text.to_string());
                    }
                }
            }
        }

        // 2. On-demand launch fallback
        let mut cdp = self.spawn_browser().await?;
        let target_url = match marketplace {
            Marketplace::Ozon => "https://www.ozon.ru/",
            Marketplace::Wildberries => "https://www.wildberries.ru/",
            Marketplace::Dns => "https://www.dns-shop.ru/",
            Marketplace::Citilink => "https://www.citilink.ru/",
            _ => "about:blank",
        };

        let session_id = cdp.create_and_attach_tab(target_url).await?;
        if marketplace == Marketplace::Ozon {
            info!("Waiting 10s quietly for Variti PoW calculation on on-demand launch...");
            tokio::time::sleep(Duration::from_secs(10)).await;
        } else {
            // Wait for document to stabilize
            let start_wait = std::time::Instant::now();
            while start_wait.elapsed() < Duration::from_millis(3000) {
                let ready_val = cdp
                    .send_target_cmd(
                        &session_id,
                        "Runtime.evaluate",
                        json!({ "expression": check_ready_expr, "returnByValue": true }),
                    )
                    .await;
                if let Ok(val) = ready_val
                    && val["result"]["result"]["value"].as_bool() == Some(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        let res = cdp
            .send_target_cmd(
                &session_id,
                "Runtime.evaluate",
                json!({
                    "expression": expr,
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;

        cdp.close().await;

        let status = res["result"]["result"]["value"]["status"]
            .as_u64()
            .unwrap_or(0);
        let ok = res["result"]["result"]["value"]["ok"]
            .as_bool()
            .unwrap_or(false);
        let text = res["result"]["result"]["value"]["text"]
            .as_str()
            .unwrap_or_default();

        if !ok || status != 200 {
            return Err(ScraperError::WafBlocked {
                waf: match marketplace {
                    Marketplace::Ozon => WafProtection::Variti,
                    Marketplace::Wildberries => WafProtection::Wbaas,
                    _ => WafProtection::Kasada,
                },
                details: format!(
                    "CDP fetch failed in {} tab: HTTP status={}, text={}",
                    marketplace, status, text
                ),
            });
        }

        Ok(text.to_string())
    }
```

- [ ] **Step 4: Run tests to verify solver and wb tests pass**

Run: `cargo test -p ru-marketplace-solver -p ru-marketplace-wb -j 2`
Expected: PASS.

---

### Task 5: End-to-End Workspace Verification & MCP Integration Tests (`crates/mcp`)

**Files:**
- Verify: All crates in Cargo workspace
- Verify: `Cargo.lock`
- Update: `dist/ru-marketplace-mcp`

- [ ] **Step 1: Run full test suite across all workspace crates**

Run: `cargo test -j 2`
Expected: PASS across all crates (`core`, `solver`, `dns`, `citilink`, `ozon`, `wb`, `mcp`).

- [ ] **Step 2: Run clippy and format checks**

Run: `cargo clippy -j 2 --all-targets -- -D warnings`
Run: `cargo fmt --check`
Expected: PASS with 0 warnings and clean formatting.

- [ ] **Step 3: Compile release binary and update `dist/`**

Run: `cargo build --release -p ru-marketplace-mcp -j 2`
Run: `cp target/release/ru-marketplace-mcp dist/ru-marketplace-mcp`
Expected: Updated standalone executable in `dist/ru-marketplace-mcp`.

- [ ] **Step 4: Verify Nix flake build**

Run: `nix build .#default --no-link`
Expected: Clean Nix build without errors.
