use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
};

/// Dynamic Basket CDN host resolver with thread-safe caching and concurrent probing.
#[derive(Debug, Clone)]
pub struct BasketResolver {
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<u64, String>>>,
}

impl Default for BasketResolver {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self::new(client)
    }
}

impl BasketResolver {
    /// Create a new resolver with an HTTP client.
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Calculate the volume partition index (nm / 100_000).
    pub fn calculate_vol(nm: u64) -> u64 {
        nm / 100_000
    }

    /// Calculate the part partition index (nm / 1_000).
    pub fn calculate_part(nm: u64) -> u64 {
        nm / 1_000
    }

    /// Prepopulate or manually set a host for a volume (useful for unit tests and warmup).
    pub async fn set_cached_host(&self, vol: u64, host: impl Into<String>) {
        let mut cache = self.cache.write().await;
        cache.insert(vol, host.into());
    }

    /// Inspect cached host for a volume without network probing.
    pub async fn get_cached_host(&self, vol: u64) -> Option<String> {
        let cache = self.cache.read().await;
        cache.get(&vol).cloned()
    }

    /// Resolve the serving CDN basket host for a specific article.
    pub async fn resolve_host(&self, vol: u64, part: u64, nm: u64) -> Result<String, ScraperError> {
        // Fast path: cached host
        if let Some(host) = self.get_cached_host(vol).await {
            debug!("Basket host cache hit for vol {vol}: {host}");
            return Ok(host);
        }

        debug!("Basket host cache miss for vol {vol}. Probing basket hosts...");
        let probe_path = format!("/vol{vol}/part{part}/{nm}/info/ru/card.json");

        let mut join_set = tokio::task::JoinSet::new();

        for i in 1..=40 {
            let host = format!("basket-{i:02}.wbbasket.ru");
            let url = format!("https://{host}{probe_path}");
            let client = self.client.clone();

            join_set.spawn(async move {
                let res = client
                    .head(&url)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await;

                match res {
                    Ok(resp) if resp.status() == reqwest::StatusCode::OK => Some(host),
                    Ok(_) => None,
                    Err(err) => {
                        debug!("Probe error on {url}: {err}");
                        None
                    }
                }
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok(Some(found_host)) => {
                    join_set.abort_all();
                    debug!("Resolved basket host for vol {vol} -> {found_host}");
                    let mut cache = self.cache.write().await;
                    cache.insert(vol, found_host.clone());
                    return Ok(found_host);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!("JoinSet task panicked or was aborted: {err}");
                }
            }
        }

        Err(ScraperError::NotFound {
            query: format!("Wildberries CDN basket not found for nm {nm}"),
        })
    }

    /// Construct the full base URL for an article's basket directory, e.g.:
    /// `https://basket-11.wbbasket.ru/vol1643/part164379/164379765/info`
    pub async fn basket_base(&self, nm: u64) -> Result<String, ScraperError> {
        let vol = Self::calculate_vol(nm);
        let part = Self::calculate_part(nm);
        let host = self.resolve_host(vol, part, nm).await?;
        Ok(format!("https://{host}/vol{vol}/part{part}/{nm}/info"))
    }

    /// Fetch and parse `/info/ru/card.json` from the resolved CDN basket shard directly.
    pub async fn get_card(&self, nm: u64) -> Result<ProductOffer, ScraperError> {
        let base = self.basket_base(nm).await?;
        let url = format!("{base}/ru/card.json");

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(ScraperError::Network)?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ScraperError::NotFound {
                query: nm.to_string(),
            });
        }

        if !status.is_success() {
            return Err(ScraperError::Parse(format!(
                "Failed to fetch card from CDN: HTTP {status}"
            )));
        }

        let body = resp.text().await.map_err(ScraperError::Network)?;
        let root: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| ScraperError::Parse(format!("Failed to parse card.json: {e}")))?;

        let title = root
            .get("imt_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();

        let seller = root
            .get("selling")
            .and_then(|s| s.get("brand_name"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Try to fetch price from price-history.json if present
        let price_url = format!("{base}/price-history.json");
        let price_tier = if let Ok(resp) = self.client.get(&price_url).send().await
            && resp.status().is_success()
            && let Ok(text) = resp.text().await
            && let Ok(arr) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(points) = arr.as_array()
            && let Some(last_point) = points.last()
            && let Some(rub_kopecks) = last_point
                .get("price")
                .and_then(|p| p.get("RUB"))
                .and_then(|v| v.as_f64())
        {
            PriceTier::Base((rub_kopecks / 100.0) as u64)
        } else {
            PriceTier::Base(0)
        };

        Ok(ProductOffer {
            marketplace: Marketplace::Wildberries,
            title: if title.is_empty() {
                format!("Wildberries Article {nm}")
            } else {
                title
            },
            url: format!("https://www.wildberries.ru/catalog/{nm}/detail.aspx"),
            price_tier,
            original_price: None,
            currency: Currency::Rub,
            rating: None,
            review_count: None,
            stock: StockStatus::InStock(1),
            seller,
            product_id: Some(nm.to_string()),
        })
    }
}
