use clap::Parser;
use std::str::FromStr;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use ru_marketplace_mcp::domain::Marketplace;
use ru_marketplace_mcp::mcp::McpServer;
use ru_marketplace_mcp::services::AggregatorService;

#[derive(Parser, Debug)]
#[command(
    name = "ru-marketplace-mcp",
    author = "RikaKit2",
    version = env!("CARGO_PKG_VERSION"),
    about = "Russian Marketplaces Price Comparison & MCP Server (Wildberries, Ozon, DNS, Citilink)"
)]
struct Cli {
    /// Run in MCP Stdio JSON-RPC 2.0 mode (default when no arguments provided)
    #[arg(long, default_value_t = false)]
    stdio: bool,

    /// Compare prices across all marketplaces for a product query
    #[arg(short, long)]
    compare: Option<String>,

    /// Filter to a specific marketplace (e.g. wildberries, ozon, dns, citilink)
    #[arg(short, long)]
    marketplace: Option<String>,

    /// Maximum results per marketplace
    #[arg(short, long, default_value_t = 5)]
    limit: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // CRITICAL: MCP stdio transport requires stdout to be pure JSON-RPC!
    // All logging MUST be directed strictly to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Direct comparison mode from CLI
    if let Some(query) = cli.compare {
        info!("Running price comparison for query: '{query}'");
        let aggregator = AggregatorService::with_defaults();

        let filter_marketplace = if let Some(m_str) = cli.marketplace {
            match Marketplace::from_str(&m_str) {
                Ok(m) => Some(vec![m]),
                Err(err) => {
                    error!("{err}");
                    std::process::exit(1);
                }
            }
        } else {
            None
        };

        let result = aggregator
            .compare_prices(&query, cli.limit, filter_marketplace.as_deref())
            .await;

        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Default mode: MCP Stdio server
    let server = McpServer::new();
    server.run_stdio().await?;

    Ok(())
}
