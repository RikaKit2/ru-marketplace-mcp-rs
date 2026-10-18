use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::sync::RwLock;
use tracing::debug;

use ru_marketplace_core::domain::{
    Currency, Marketplace, PriceTier, ProductOffer, ScraperError, StockStatus,
};

/// Predict the Wildberries CDN basket shard host for a volume index based on historical partition ranges.
pub fn basket_host_for_vol(vol: u64) -> &'static str {
    const TABLE: &[(u64, &str)] = &[
        (143, "basket-01.wbbasket.ru"),
        (287, "basket-02.wbbasket.ru"),
        (431, "basket-03.wbbasket.ru"),
        (719, "basket-04.wbbasket.ru"),
        (1007, "basket-05.wbbasket.ru"),
        (1061, "basket-06.wbbasket.ru"),
        (1115, "basket-07.wbbasket.ru"),
        (1169, "basket-08.wbbasket.ru"),
        (1313, "basket-09.wbbasket.ru"),
        (1601, "basket-10.wbbasket.ru"),
        (1655, "basket-11.wbbasket.ru"),
        (1919, "basket-12.wbbasket.ru"),
        (2045, "basket-13.wbbasket.ru"),
        (2189, "basket-14.wbbasket.ru"),
        (2405, "basket-15.wbbasket.ru"),
        (2621, "basket-16.wbbasket.ru"),
        (2837, "basket-17.wbbasket.ru"),
        (3053, "basket-18.wbbasket.ru"),
        (3269, "basket-19.wbbasket.ru"),
        (3485, "basket-20.wbbasket.ru"),
        (3701, "basket-21.wbbasket.ru"),
        (3917, "basket-22.wbbasket.ru"),
        (4133, "basket-23.wbbasket.ru"),
        (4349, "basket-24.wbbasket.ru"),
        (4565, "basket-25.wbbasket.ru"),
        (4781, "basket-26.wbbasket.ru"),
        (4997, "basket-27.wbbasket.ru"),
    ];
    for &(upper, host) in TABLE {
        if vol <= upper {
            return host;
        }
    }
    "basket-28.wbbasket.ru"
}

/// Dynamic Basket CDN host resolver with thread-safe caching and concurrent probing.
#[derive(Debug, Clone)]
pub struct BasketResolver {
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<u64, String>>>,
    max_known_basket: Arc<AtomicU16>,
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
            max_known_basket: Arc::new(AtomicU16::new(60)),
        }
    }

    /// Access internal reqwest client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Calculate the volume partition index (nm / 100_000).
    pub fn calculate_vol(nm: u64) -> u64 {
        nm / 100_000
    }

    /// Calculate the part partition index (nm / 1_000).
    pub fn calculate_part(nm: u64) -> u64 {
        nm / 1_000
    }

    /// Get current maximum known basket shard ceiling.
    pub fn max_known_basket(&self) -> u16 {
        self.max_known_basket.load(Ordering::Relaxed)
    }

    /// Concurrently probe a window of basket hosts from start_idx to end_idx.
    async fn probe_window(
        client: &reqwest::Client,
        start_idx: u16,
        end_idx: u16,
        probe_path: &str,
    ) -> Option<(u16, String)> {
        let mut join_set = tokio::task::JoinSet::new();

        for i in start_idx..=end_idx {
            let host = format!("basket-{i:02}.wbbasket.ru");
            let url = format!("https://{host}{probe_path}");
            let cl = client.clone();

            join_set.spawn(async move {
                let res = cl
                    .head(&url)
                    .timeout(std::time::Duration::from_secs(4))
                    .send()
                    .await;

                match res {
                    Ok(resp) if resp.status() == reqwest::StatusCode::OK => Some((i, host)),
                    _ => None,
                }
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            if let Ok(Some((idx, found_host))) = join_result {
                join_set.abort_all();
                return Some((idx, found_host));
            }
        }

        None
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

        let probe_path = format!("/vol{vol}/part{part}/{nm}/info/ru/card.json");

        // Fast-path: table-predicted basket shard
        let candidate = basket_host_for_vol(vol);
        let candidate_url = format!("https://{candidate}{probe_path}");
        if let Ok(resp) = self
            .client
            .head(&candidate_url)
            .timeout(std::time::Duration::from_millis(800))
            .send()
            .await
            && resp.status() == reqwest::StatusCode::OK
        {
            debug!("Resolved basket host via static mapping for vol {vol} -> {candidate}");
            let mut cache = self.cache.write().await;
            cache.insert(vol, candidate.to_string());
            return Ok(candidate.to_string());
        }

        debug!("Basket host cache miss for vol {vol}. Probing basket hosts...");
        let current_max = self.max_known_basket.load(Ordering::Relaxed);

        // Window 1: probe known range 1..=current_max
        if let Some((found_idx, found_host)) =
            Self::probe_window(&self.client, 1, current_max, &probe_path).await
        {
            debug!("Resolved basket host for vol {vol} -> {found_host} (index {found_idx})");
            self.max_known_basket
                .fetch_max(found_idx, Ordering::Relaxed);
            let mut cache = self.cache.write().await;
            cache.insert(vol, found_host.clone());
            return Ok(found_host);
        }

        // Window 2: Adaptive expansion up to (current_max + 25) capped at 99
        let expansion_end = (current_max + 25).min(99);
        if expansion_end > current_max {
            debug!(
                "Probing adaptive expansion window {}..={} for vol {vol}...",
                current_max + 1,
                expansion_end
            );
            if let Some((found_idx, found_host)) =
                Self::probe_window(&self.client, current_max + 1, expansion_end, &probe_path).await
            {
                debug!(
                    "Resolved basket host via expansion for vol {vol} -> {found_host} (new max: {found_idx})"
                );
                self.max_known_basket
                    .fetch_max(found_idx, Ordering::Relaxed);
                let mut cache = self.cache.write().await;
                cache.insert(vol, found_host.clone());
                return Ok(found_host);
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

        let resp = ru_marketplace_core::transport::send_with_transient_retry(
            2,
            std::time::Duration::from_millis(300),
            || self.client.get(&url).send(),
        )
        .await?;

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
        let price_tier = if let Ok(resp) =
            ru_marketplace_core::transport::send_with_transient_retry(
                1,
                std::time::Duration::from_millis(200),
                || self.client.get(&price_url).send(),
            )
            .await
            && resp.status().is_success()
            && let Ok(text) = resp.text().await
            && let Ok(arr) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(points) = arr.as_array()
            && let Some(last_point) = points.last()
            && let Some(rub_kopecks) = last_point
                .get("price")
                .and_then(|p| p.get("RUB").or_else(|| p.get("rub")))
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
