use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use ru_marketplace_core::connector::MarketplaceConnector;
use ru_marketplace_core::domain::{
    Marketplace, ProductOffer, ScraperError, SearchFilterOptions, SortOrder,
};
use ru_marketplace_core::session::SessionManager;
use ru_marketplace_solver::NativeCdpSolver;

use super::basket::BasketResolver;
use super::models::{
    WbCategoriesResponse, WbCategoryProductsResponse, WbReviewsResponse, WbRootInfo, WbSellerInfo,
};
use super::parser::{
    parse_categories, parse_products_json, parse_reviews_json, parse_root_info_json,
    parse_seller_info_json,
};

pub const WB_DEFAULT_DEST: &str = "1259570991";
pub const WB_SEARCH_URL: &str =
    "https://www.wildberries.ru/__internal/u-search/exactmatch/ru/common/v18/search";
pub const WB_CARD_URL: &str = "https://card.wb.ru/cards/v4/detail";

pub const WB_REVIEW_HOSTS: &[&str] = &["feedbacks2.wb.ru", "feedbacks1.wb.ru"];
pub const WB_STATIC_HOSTS: &[&str] = &[
    "static-basket-01.wbbasket.ru",
    "static-basket-02.wbbasket.ru",
    "basket-01.wbbasket.ru",
];
type MenuCache = Option<(Vec<serde_json::Value>, String)>;

/// Wildberries marketplace catalog connector with CDN shard resolving and solver search.
#[derive(Debug, Clone)]
pub struct WildberriesConnector {
    dest: String,
    basket_resolver: BasketResolver,
    session_manager: Arc<SessionManager>,
    solver: Arc<NativeCdpSolver>,
    client: reqwest::Client,
    menu_cache: Arc<tokio::sync::RwLock<MenuCache>>,
}

impl Default for WildberriesConnector {
    fn default() -> Self {
        Self::new(WB_DEFAULT_DEST)
    }
}

impl WildberriesConnector {
    pub fn new(dest: impl Into<String>) -> Self {
        let solver = Arc::new(NativeCdpSolver::default());
        let session_manager = Arc::new(SessionManager::default());
        Self::with_components(dest, session_manager, solver)
    }

    pub fn with_components(
        dest: impl Into<String>,
        session_manager: Arc<SessionManager>,
        solver: Arc<NativeCdpSolver>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            dest: dest.into(),
            basket_resolver: BasketResolver::new(client.clone()),
            session_manager,
            solver,
            client,
            menu_cache: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    pub fn basket_resolver(&self) -> &BasketResolver {
        &self.basket_resolver
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Batch retrieval of product cards up to 100 SKUs per request.
    /// First attempts direct HTTP with browser headers, falling back to browser CDP on block.
    pub async fn get_cards_batch(&self, nm_ids: &[u64]) -> Result<Vec<ProductOffer>, ScraperError> {
        if nm_ids.is_empty() {
            return Ok(Vec::new());
        }

        if nm_ids.len() > 100 {
            return Err(ScraperError::InvalidFilter(format!(
                "Batch card request exceeds maximum limit of 100 items: got {}",
                nm_ids.len()
            )));
        }

        let nm_param = nm_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(";");

        let card_url = format!(
            "{}?appType=1&curr=rub&dest={}&locale=ru&spp=30&nm={nm_param}",
            WB_CARD_URL, self.dest
        );

        // Smart Tiering: try direct HTTP first with browser headers
        let direct_res = ru_marketplace_core::transport::send_with_transient_retry(
            1,
            std::time::Duration::from_millis(300),
            || {
                self.client
                    .get(&card_url)
                    .header(
                        "User-Agent",
                        ru_marketplace_core::transport::DEFAULT_DESKTOP_UA,
                    )
                    .header("Accept", "*/*")
                    .header("Accept-Language", "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7")
                    .send()
            },
        )
        .await;

        let body = match direct_res {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await.map_err(ScraperError::Network)?;
                if text.trim_start().starts_with("<html") {
                    // WAF HTML challenge returned with 200 OK -> fallback to browser solver
                    self.solver
                        .fetch_in_tab(Marketplace::Wildberries, &card_url)
                        .await?
                } else {
                    text
                }
            }
            _ => {
                // Fallback to browser solver tab
                self.solver
                    .fetch_in_tab(Marketplace::Wildberries, &card_url)
                    .await?
            }
        };

        parse_products_json(&body, nm_ids.len())
    }

    /// Retrieve root product information and model-line identifier (imt_id) from basket CDN.
    pub async fn get_root_info(&self, nm_id: u64) -> Result<WbRootInfo, ScraperError> {
        let vol = BasketResolver::calculate_vol(nm_id);
        let part = BasketResolver::calculate_part(nm_id);
        let host = self.basket_resolver.resolve_host(vol, part, nm_id).await?;
        let url = format!("https://{host}/vol{vol}/part{part}/{nm_id}/info/ru/card.json");

        let resp = ru_marketplace_core::transport::send_with_transient_retry(
            2,
            std::time::Duration::from_millis(300),
            || self.client.get(&url).send(),
        )
        .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ScraperError::NotFound {
                query: format!("basket card.json not found for nm {nm_id}"),
            });
        }

        if !status.is_success() {
            return Err(ScraperError::Parse(format!(
                "Failed to fetch root info from CDN: HTTP {status}"
            )));
        }

        let body = resp.text().await.map_err(ScraperError::Network)?;
        parse_root_info_json(&body, nm_id, Some(host))
    }

    /// Retrieve customer reviews and feedback by model-line root ID (imt_id) with pagination.
    pub async fn get_reviews(
        &self,
        imt_id: u64,
        page: usize,
    ) -> Result<WbReviewsResponse, ScraperError> {
        for &host in WB_REVIEW_HOSTS {
            let url = format!("https://{host}/feedbacks/v2/{imt_id}");
            let res = self
                .client
                .get(&url)
                .header(
                    "User-Agent",
                    ru_marketplace_core::transport::DEFAULT_DESKTOP_UA,
                )
                .header("Accept", "*/*")
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.text().await.map_err(ScraperError::Network)?;
                    return parse_reviews_json(&body, imt_id, page, Some(host.to_string()));
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    // Try next mirror or report not found
                    continue;
                }
                _ => continue,
            }
        }

        Err(ScraperError::NotFound {
            query: format!("Wildberries reviews not found for imt_id {imt_id}"),
        })
    }

    /// Retrieve registered legal entity details for a Wildberries supplier/seller.
    pub async fn get_seller_info(&self, supplier_id: u64) -> Result<WbSellerInfo, ScraperError> {
        for &host in WB_STATIC_HOSTS {
            let url = format!("https://{host}/vol0/data/supplier-by-id/{supplier_id}.json");
            let res = self
                .client
                .get(&url)
                .header(
                    "User-Agent",
                    ru_marketplace_core::transport::DEFAULT_DESKTOP_UA,
                )
                .header("Accept", "*/*")
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.text().await.map_err(ScraperError::Network)?;
                    if let Ok(info) =
                        parse_seller_info_json(&body, supplier_id, Some(host.to_string()))
                    {
                        return Ok(info);
                    }
                }
                _ => continue,
            }
        }

        Err(ScraperError::NotFound {
            query: format!("Wildberries seller info not found for supplier_id {supplier_id}"),
        })
    }

    /// Pre-populate the in-memory catalog menu cache (e.g. for testing or offline environments).
    pub async fn set_menu_cache(&self, menu: Vec<serde_json::Value>, host: String) {
        let mut cache = self.menu_cache.write().await;
        *cache = Some((menu, host));
    }

    /// Retrieve the main WB catalog menu, using in-memory cache or fetching from static mirrors.
    pub async fn get_menu(&self) -> Result<(Vec<serde_json::Value>, String), ScraperError> {
        // Fast path: check memory cache under read lock
        {
            let cache = self.menu_cache.read().await;
            if let Some((menu, host)) = cache.as_ref() {
                return Ok((menu.clone(), host.clone()));
            }
        }

        // Cache miss: fetch from static mirrors
        let mut last_error = String::new();
        for &host in WB_STATIC_HOSTS {
            let url = format!("https://{host}/vol0/data/main-menu-ru-ru-v3.json");
            let res = self
                .client
                .get(&url)
                .header(
                    "User-Agent",
                    ru_marketplace_core::transport::DEFAULT_DESKTOP_UA,
                )
                .header("Accept", "*/*")
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.text().await.map_err(ScraperError::Network)?;
                    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                        ScraperError::Parse(format!("Invalid WB menu JSON from {host}: {e}"))
                    })?;
                    let nodes = match parsed {
                        serde_json::Value::Array(arr) => arr,
                        _ => {
                            return Err(ScraperError::Parse(
                                "WB menu expected a top-level JSON array".to_string(),
                            ));
                        }
                    };

                    let mut cache = self.menu_cache.write().await;
                    *cache = Some((nodes.clone(), host.to_string()));
                    return Ok((nodes, host.to_string()));
                }
                Ok(resp) => {
                    last_error = format!("HTTP {} from {host}", resp.status());
                }
                Err(e) => {
                    last_error = format!("Network error connecting to {host}: {e}");
                }
            }
        }

        Err(ScraperError::Parse(format!(
            "Failed to load WB catalog menu from all mirrors: {last_error}"
        )))
    }

    /// Browse the Wildberries catalog category tree with depth and subtree filtering.
    pub async fn get_categories(
        &self,
        root: &str,
        max_depth: usize,
    ) -> Result<WbCategoriesResponse, ScraperError> {
        let (menu, host) = self.get_menu().await?;
        parse_categories(&menu, root, max_depth, Some(host))
    }

    /// Retrieve products from a category feed specified by shard and query.
    pub async fn get_category_products(
        &self,
        shard: &str,
        query: &str,
        page: usize,
        sort: Option<&str>,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let resp = self
            .get_category_products_full(shard, query, page, sort, None)
            .await?;
        Ok(resp.items)
    }

    /// Retrieve full page of products from a category feed with pagination metadata.
    pub async fn get_category_products_full(
        &self,
        shard: &str,
        query: &str,
        page: usize,
        sort: Option<&str>,
        dest: Option<&str>,
    ) -> Result<WbCategoryProductsResponse, ScraperError> {
        let shard_norm = shard.trim();
        let query_norm = query.trim().trim_start_matches(['?', '&']);

        if shard_norm.is_empty() {
            return Err(ScraperError::InvalidFilter(
                "shard is required — obtain it from get_categories".to_string(),
            ));
        }

        if query_norm.is_empty() {
            return Err(ScraperError::InvalidFilter(
                "query is required — obtain it from get_categories, e.g. 'cat=9845'".to_string(),
            ));
        }

        if shard_norm.eq_ignore_ascii_case("blackhole") {
            return Err(ScraperError::InvalidFilter(
                "Категория имеет шард 'blackhole' и является только навигационной, не имея прямого листинга товаров (category shard 'blackhole' is navigation-only with no direct product listing). Expand subcategories via get_categories or use search instead.".to_string(),
            ));
        }

        let page = page.max(1);
        let sort_str = sort.unwrap_or("popular").trim();
        let dest_str = dest.unwrap_or(&self.dest).trim();
        let dest_used = if dest_str.is_empty() {
            &self.dest
        } else {
            dest_str
        };

        let url = format!(
            "https://catalog.wb.ru/catalog/{shard_norm}/v4/catalog?appType=1&curr=rub&dest={dest_used}&locale=ru&page={page}&sort={sort_str}&spp=30&{query_norm}"
        );

        // Smart Tiering: direct HTTP first, fallback to browser solver
        let direct_res = ru_marketplace_core::transport::send_with_transient_retry(
            1,
            std::time::Duration::from_millis(300),
            || {
                self.client
                    .get(&url)
                    .header(
                        "User-Agent",
                        ru_marketplace_core::transport::DEFAULT_DESKTOP_UA,
                    )
                    .header("Accept", "*/*")
                    .header("Accept-Language", "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7")
                    .send()
            },
        )
        .await;

        let body = match direct_res {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await.map_err(ScraperError::Network)?;
                if text.trim_start().starts_with("<html") {
                    // WAF HTML challenge returned with 200 OK -> fallback to browser solver
                    self.solver
                        .fetch_in_tab(Marketplace::Wildberries, &url)
                        .await?
                } else {
                    text
                }
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                return Err(ScraperError::NotFound {
                    query: format!(
                        "No category feed at shard '{shard_norm}' for '{query_norm}' — selectors change with the menu"
                    ),
                });
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                return Err(ScraperError::RateLimited {
                    retry_after_secs: Some(30),
                });
            }
            _ => {
                // Fallback to browser solver tab
                self.solver
                    .fetch_in_tab(Marketplace::Wildberries, &url)
                    .await?
            }
        };

        let items = parse_products_json(&body, 100)?;
        let count = items.len();
        let has_more = count >= 100;

        Ok(WbCategoryProductsResponse {
            shard: shard_norm.to_string(),
            query: query_norm.to_string(),
            page,
            sort: sort_str.to_string(),
            dest: dest_used.to_string(),
            count,
            has_more,
            items,
        })
    }
}

#[async_trait]
impl MarketplaceConnector for WildberriesConnector {
    fn marketplace(&self) -> Marketplace {
        Marketplace::Wildberries
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilterOptions,
    ) -> Result<Vec<ProductOffer>, ScraperError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let mut query_params: Vec<(&str, String)> = vec![
            ("appType", "1".to_string()),
            ("curr", "rub".to_string()),
            ("dest", self.dest.clone()),
            ("locale", "ru".to_string()),
            ("query", trimmed.to_string()),
            ("resultset", "catalog".to_string()),
            ("page", "1".to_string()),
            ("spp", "30".to_string()),
        ];

        match (filters.min_price, filters.max_price) {
            (Some(min), Some(max)) => {
                query_params.push(("priceU", format!("{};{}", min * 100, max * 100)));
            }
            (Some(min), None) => {
                query_params.push(("priceU", format!("{};", min * 100)));
            }
            (None, Some(max)) => {
                query_params.push(("priceU", format!(";{}", max * 100)));
            }
            (None, None) => {}
        }

        if let Some(sort) = filters.sort {
            match sort {
                SortOrder::PriceAsc => query_params.push(("sort", "priceup".to_string())),
                SortOrder::PriceDesc => query_params.push(("sort", "pricedown".to_string())),
                SortOrder::Rating => query_params.push(("sort", "rate".to_string())),
                SortOrder::Discount => query_params.push(("sort", "benefit".to_string())),
                SortOrder::Popular => query_params.push(("sort", "popular".to_string())),
                SortOrder::Reviews => {}
            }
        }

        let mut url =
            reqwest::Url::parse(WB_SEARCH_URL).map_err(|e| ScraperError::Parse(e.to_string()))?;
        for (k, v) in query_params {
            url.query_pairs_mut().append_pair(k, &v);
        }
        let search_url = url.to_string();

        // Execute search strictly via browser tab to ensure genuine Chrome TLS Client Hello
        let body = self
            .solver
            .fetch_in_tab(Marketplace::Wildberries, &search_url)
            .await?;

        let mut offers = parse_products_json(&body, limit)?;

        if filters.sort == Some(SortOrder::Reviews) {
            offers.sort_by_key(|b| std::cmp::Reverse(b.review_count.unwrap_or(0)));
        }

        if let Some(min) = filters.min_price {
            offers.retain(|o| o.price_tier.price() >= min);
        }
        if let Some(max) = filters.max_price {
            offers.retain(|o| o.price_tier.price() <= max);
        }

        Ok(offers)
    }

    async fn get_card(&self, item_id: &str) -> Result<ProductOffer, ScraperError> {
        let id_clean = item_id.trim();
        let nm = id_clean
            .parse::<u64>()
            .map_err(|_| ScraperError::NotFound {
                query: id_clean.to_string(),
            })?;

        // 1. Primary fast-path: fetch directly from CDN basket shard (50ms, zero browser)
        match self.basket_resolver.get_card(nm).await {
            Ok(card) => return Ok(card),
            Err(err) => {
                info!(
                    "CDN basket lookup missed for nm {nm}: {err}, falling back to batch cards API..."
                );
            }
        }

        // 2. Fallback: batch cards API for single nm
        let offers = self.get_cards_batch(&[nm]).await?;
        offers
            .into_iter()
            .next()
            .ok_or_else(|| ScraperError::NotFound {
                query: id_clean.to_string(),
            })
    }
}
