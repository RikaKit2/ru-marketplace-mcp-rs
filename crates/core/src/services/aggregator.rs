use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

use crate::connector::MarketplaceConnector;
use crate::domain::{Currency, Marketplace, ProductOffer, SearchFilterOptions, SortOrder};

/// Outcome report for a single marketplace query attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceOutcome {
    /// Marketplace queried.
    pub source: Marketplace,
    /// Status: "ok", "blocked", "rate_limited", "timeout", "error".
    pub status: String,
    /// Detailed message or error explanation.
    pub detail: String,
    /// Number of offers returned by this marketplace.
    pub offers_returned: usize,
    /// Elapsed time for this source in milliseconds.
    pub elapsed_ms: u64,
    /// Warnings or notices collected.
    pub warnings: Vec<String>,
}

/// Aggregated multi-source price comparison response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareResult {
    /// Searched query.
    pub query: String,
    /// Marketplaces that were attempted.
    pub sources_queried: Vec<Marketplace>,
    /// Marketplaces that responded successfully.
    pub sources_ok: Vec<Marketplace>,
    /// True only if all queried marketplaces responded successfully.
    pub complete: bool,
    /// Total number of offers collected.
    pub total_offers: usize,
    /// Overall cheapest offer across all marketplaces (based on everyday RUB price).
    pub cheapest: Option<ProductOffer>,
    /// Difference between highest and lowest RUB price among offers.
    pub price_spread_rub: Option<u64>,
    /// All offers sorted by price in ascending order.
    pub offers: Vec<ProductOffer>,
    /// Per-source execution reports.
    pub source_outcomes: Vec<SourceOutcome>,
    /// Global comparison warnings.
    pub warnings: Vec<String>,
}

/// Service orchestrating concurrent marketplace searches and ranking offers.
#[derive(Clone)]
pub struct AggregatorService {
    connectors: Vec<Arc<dyn MarketplaceConnector>>,
}

impl AggregatorService {
    /// Create an aggregator with connector instances.
    pub fn new(connectors: Vec<Arc<dyn MarketplaceConnector>>) -> Self {
        Self { connectors }
    }

    /// Retrieve connector for a specific marketplace.
    pub fn get_connector(&self, marketplace: Marketplace) -> Option<Arc<dyn MarketplaceConnector>> {
        self.connectors
            .iter()
            .find(|c| c.marketplace() == marketplace)
            .cloned()
    }

    /// Compare prices across configured marketplaces concurrently.
    pub async fn compare_prices(
        &self,
        query: &str,
        per_source_limit: usize,
        filter_sources: Option<&[Marketplace]>,
        filters: Option<&SearchFilterOptions>,
    ) -> CompareResult {
        let query_trimmed = query.trim().to_string();
        let mut join_set = JoinSet::new();

        let mut sources_queried = Vec::new();
        let default_filters = SearchFilterOptions::default();
        let active_filters = filters.cloned().unwrap_or(default_filters);

        for connector in &self.connectors {
            let mkt = connector.marketplace();
            if let Some(filter) = filter_sources
                && !filter.contains(&mkt)
            {
                continue;
            }

            sources_queried.push(mkt);
            let conn = Arc::clone(connector);
            let q = query_trimmed.clone();
            let f = active_filters.clone();

            join_set.spawn(async move {
                let start = Instant::now();
                let res = conn.search(&q, per_source_limit, &f).await;
                let elapsed_ms = start.elapsed().as_millis() as u64;
                (mkt, res, elapsed_ms)
            });
        }

        let mut all_offers = Vec::new();
        let mut source_outcomes = Vec::new();
        let mut sources_ok = Vec::new();
        let mut global_warnings = Vec::new();

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok((mkt, result, elapsed_ms)) => match result {
                    Ok(offers) => {
                        let count = offers.len();
                        sources_ok.push(mkt);
                        all_offers.extend(offers);
                        source_outcomes.push(SourceOutcome {
                            source: mkt,
                            status: "ok".to_string(),
                            detail: format!("Found {count} offer(s)"),
                            offers_returned: count,
                            elapsed_ms,
                            warnings: Vec::new(),
                        });
                    }
                    Err(err) => {
                        let status = err.status_tag().to_string();
                        let detail = err.to_string();
                        let warning_msg = format!("Source '{mkt}' failed: {detail}");
                        global_warnings.push(warning_msg.clone());

                        source_outcomes.push(SourceOutcome {
                            source: mkt,
                            status,
                            detail,
                            offers_returned: 0,
                            elapsed_ms,
                            warnings: vec![warning_msg],
                        });
                    }
                },
                Err(join_err) => {
                    global_warnings.push(format!("Task execution error: {join_err}"));
                }
            }
        }

        // Apply post-aggregation price filtering
        if let Some(f) = filters {
            if let Some(min) = f.min_price {
                all_offers.retain(|o| o.price_tier.price() >= min);
            }
            if let Some(max) = f.max_price {
                all_offers.retain(|o| o.price_tier.price() <= max);
            }
        }

        // Sort offers based on requested criteria or default to price ascending
        if let Some(f) = filters
            && let Some(sort) = f.sort
        {
            match sort {
                SortOrder::PriceAsc => {
                    all_offers.sort_by(|a, b| match (a.rank_price_rub(), b.rank_price_rub()) {
                        (Some(pa), Some(pb)) => pa.cmp(&pb),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.title.cmp(&b.title),
                    });
                }
                SortOrder::PriceDesc => {
                    all_offers.sort_by(|a, b| match (a.rank_price_rub(), b.rank_price_rub()) {
                        (Some(pa), Some(pb)) => pb.cmp(&pa),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => b.title.cmp(&a.title),
                    });
                }
                SortOrder::Rating => {
                    all_offers.sort_by(|a, b| {
                        b.rating
                            .unwrap_or(0.0)
                            .partial_cmp(&a.rating.unwrap_or(0.0))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                SortOrder::Reviews => {
                    all_offers.sort_by(|a, b| {
                        b.review_count
                            .unwrap_or(0)
                            .cmp(&a.review_count.unwrap_or(0))
                    });
                }
                SortOrder::Discount | SortOrder::Popular => {
                    all_offers.sort_by(|a, b| match (a.rank_price_rub(), b.rank_price_rub()) {
                        (Some(pa), Some(pb)) => pa.cmp(&pb),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.title.cmp(&b.title),
                    });
                }
            }
        } else {
            all_offers.sort_by(|a, b| match (a.rank_price_rub(), b.rank_price_rub()) {
                (Some(pa), Some(pb)) => pa.cmp(&pb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.title.cmp(&b.title),
            });
        }

        let cheapest = all_offers
            .iter()
            .find(|o| o.currency == Currency::Rub)
            .cloned();

        let rub_prices: Vec<u64> = all_offers
            .iter()
            .filter_map(|o| o.rank_price_rub())
            .collect();

        let price_spread_rub = if rub_prices.len() >= 2 {
            let min = rub_prices.iter().min().copied().unwrap_or(0);
            let max = rub_prices.iter().max().copied().unwrap_or(0);
            Some(max.saturating_sub(min))
        } else {
            None
        };

        let complete = sources_queried.len() == sources_ok.len();
        let total_offers = all_offers.len();

        CompareResult {
            query: query_trimmed,
            sources_queried,
            sources_ok,
            complete,
            total_offers,
            cheapest,
            price_spread_rub,
            offers: all_offers,
            source_outcomes,
            warnings: global_warnings,
        }
    }
}
