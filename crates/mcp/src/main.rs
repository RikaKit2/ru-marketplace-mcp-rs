use clap::{Parser, Subcommand};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use ru_marketplace_core::domain::Marketplace;
use ru_marketplace_core::session::SessionManager;
use ru_marketplace_mcp::McpServer;
use ru_marketplace_solver::NativeCdpSolver;

#[derive(Parser, Debug)]
#[command(
    name = "ru-marketplace-mcp",
    author = "RikaKit2",
    version = env!("CARGO_PKG_VERSION"),
    about = "Russian Marketplaces Price Comparison & MCP Server (Wildberries, Ozon, DNS, Citilink)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

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

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manually trigger anti-bot token harvest and update cookie cache
    RefreshTokens {
        /// Marketplace name (e.g. wildberries, ozon, or all)
        #[arg(short, long, default_value = "all")]
        marketplace: String,
    },
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

    // Subcommand execution
    if let Some(Commands::RefreshTokens { marketplace }) = cli.command {
        info!("Manual token refresh requested for marketplace: '{marketplace}'");
        let session_manager = SessionManager::default();
        let solver = Arc::new(NativeCdpSolver::default());
        session_manager.register_solver_sync(solver.as_session_solver(Marketplace::Ozon));
        session_manager.register_solver_sync(solver.as_session_solver(Marketplace::Wildberries));

        let targets = if marketplace.eq_ignore_ascii_case("all") {
            vec!["wildberries", "ozon"]
        } else {
            vec![marketplace.as_str()]
        };

        let mut results = Vec::new();
        for target in targets {
            match session_manager.refresh(target).await {
                Ok(session) => {
                    info!("Successfully refreshed tokens for {target}");
                    results.push(session);
                }
                Err(err) => {
                    error!("Failed to refresh tokens for {target}: {err}");
                }
            }
        }

        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    // Direct comparison mode from CLI
    if let Some(query) = cli.compare {
        info!("Running price comparison for query: '{query}'");
        let server = McpServer::new();

        let _filter_marketplace = if let Some(m_str) = cli.marketplace {
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

        let args = serde_json::json!({
            "query": query,
            "limit": cli.limit,
        });

        let result = server.call_tool("compare_prices", &args).await;
        if let Some(c) = result.content.first() {
            if result.is_error == Some(true) {
                eprintln!("{}", c.text);
            } else {
                println!("{}", c.text);
            }
        }
        return Ok(());
    }

    // Default mode: MCP Stdio server
    let server = McpServer::new();
    server.run_stdio().await?;

    Ok(())
}
