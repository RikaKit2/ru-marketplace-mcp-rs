# Cargo Workspace Refactoring & Universal Session Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the monolithic `ru-marketplace-mcp` crate into a modular Cargo Workspace (`crates/core`, `crates/solver`, `crates/ozon`, `crates/wb`, `crates/dns`, `crates/citilink`, `crates/mcp`), implement universal session tools (`session_start`, `session_stop`, `session_status`), route catalog searches through a single multi-tab Chromium solver without disabling web security, and route WB product cards through direct CDN basket shards via `BasketResolver`.

**Architecture:** A virtual Cargo Workspace (`resolver = "3"`, `default-members = ["crates/mcp"]`).
- `crates/core`: Domain models, errors, connector traits (`MarketplaceConnector`), session traits (`SessionSolver`), cookie persistence (`CookieStore`), `SessionManager`, and `AggregatorService`.
- `crates/solver`: `NativeCdpSolver` running a single Chromium process on Xvfb `:99` with standard web security (SOP & CORS enabled). Maintains isolated browser tabs for Ozon (`https://www.ozon.ru/`, same-origin requests) and Wildberries (`https://www.wildberries.ru/`, compliant with WB's `access-control-allow-origin: https://www.wildberries.ru`).
- `crates/ozon`: Ozon Composer API v2 connector and widget parser; search and card queries run via the Ozon browser tab.
- `crates/wb`: Wildberries connector; `wb_search` runs via the WB browser tab for genuine TLS Client Hello (bypassing wbaas WAF 403); `wb_card` runs directly via dynamic CDN basket shards (`basket-01..40.wbbasket.ru`) without browser for 50ms latency; `feedbacks2.wb.ru` reviews and price history run via direct HTTP.
- `crates/dns`: DNS-Shop connector with solver fallback on 401.
- `crates/citilink`: Citilink connector with solver fallback on challenge.
- `crates/mcp`: Stdio JSON-RPC 2.0 server exposing `session_start`, `session_stop`, `session_status`, and preserving `ozon_session_*` backward-compatible aliases.

**Tech Stack:** Rust 2024 edition, Cranelift backend (`-Zcodegen-backend=cranelift`), Wild linker, Tokio (`rt-multi-thread`), Reqwest (rustls), Serde / Serde JSON, Async-Trait, Async-Tungstenite, Clap, Tracing.

**Spec:** User instructions: multi-tab single Chromium solver, standard web security, `wb_search` and `ozon_search` through browser, `wb_card` through CDN shards, `session_start`/`session_stop`/`session_status` tool names.

## Global Constraints

- Must compile cleanly with Rust 2024 edition and Cranelift/native target configuration (`.cargo/config.toml`).
- All Cargo compilation and test commands MUST use `-j 2` due to host CPU limits (2 cores / 4 threads).
- Standard web security must remain intact: do NOT use `--disable-web-security`. Use dedicated tabs with appropriate origin domains.
- Stdio transport integrity: all logs strictly to `stderr`, `stdout` strictly reserved for JSON-RPC 2.0 messages.
- Zero visible windows on Niri/Wayland: Xvfb `:99` for Chromium CDP solver.
- Universal session tools named strictly: `session_start`, `session_stop`, `session_status`.
- Backward compatibility: legacy tool names `ozon_session_start` and `ozon_session_stop` must remain functional aliases in `call_tool`.
- Pre-commit hooks (`prek`, `clippy -D warnings`, `rustfmt`) must pass cleanly.
- Binary packaging: release build must update `dist/ru-marketplace-mcp` for Nix flake compatibility.

---

### Task 1: Workspace Scaffolding & Root Manifest

**Files:**
- Modify: `Cargo.toml`
- Verify: `.cargo/config.toml`

**Interfaces:**
- Produces: Root virtual manifest with workspace dependencies and member declarations:
  - `crates/core` (`ru-marketplace-core`)
  - `crates/solver` (`ru-marketplace-solver`)
  - `crates/ozon` (`ru-marketplace-ozon`)
  - `crates/wb` (`ru-marketplace-wb`)
  - `crates/dns` (`ru-marketplace-dns`)
  - `crates/citilink` (`ru-marketplace-citilink`)
  - `crates/mcp` (`ru-marketplace-mcp`)

- [ ] **Step 1: Write root virtual `Cargo.toml`**
Configure `[workspace]` with `resolver = "3"`, `default-members = ["crates/mcp"]`, and centralized `[workspace.dependencies]`:
```toml
[workspace]
resolver = "3"
default-members = ["crates/mcp"]
members = [
    "crates/core",
    "crates/solver",
    "crates/ozon",
    "crates/wb",
    "crates/dns",
    "crates/citilink",
    "crates/mcp",
]

[workspace.package]
version = "0.1.1"
edition = "2024"
authors = ["RikaKit2"]
license = "MIT"

[workspace.dependencies]
ru-marketplace-core = { path = "crates/core" }
ru-marketplace-solver = { path = "crates/solver" }
ru-marketplace-ozon = { path = "crates/ozon" }
ru-marketplace-wb = { path = "crates/wb" }
ru-marketplace-dns = { path = "crates/dns" }
ru-marketplace-citilink = { path = "crates/citilink" }

futures = "0.3.31"
async-trait = "0.1.92"
clap = { version = "4.6.7", features = ["derive", "env"] }
reqwest = { version = "0.13.5", default-features = false, features = ["json", "stream", "rustls", "query", "cookies"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
thiserror = "2.0.21"
tokio = { version = "1.53.3", features = ["rt-multi-thread", "macros", "net", "signal", "process", "time", "io-util", "io-std", "sync"] }
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter"] }
async-tungstenite = { version = "0.32.1", features = ["tokio-runtime"] }
libc = "0.2"
chromiumoxide = { version = "0.9.1", default-features = false, features = ["bytes"] }
```

- [ ] **Step 2: Create directory scaffolding for all workspace crates**
Create directory tree:
```
crates/core/src
crates/solver/src
crates/ozon/src
crates/wb/src
crates/dns/src
crates/citilink/src
crates/mcp/src
```

- [ ] **Step 3: Verify workspace layout with cargo metadata**
Run: `cargo metadata --format-version 1`

---

### Task 2: Implement `crates/core` (`ru-marketplace-core`)

**Files:**
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`
- Create: `crates/core/src/domain/mod.rs`
- Create: `crates/core/src/domain/marketplaces.rs`
- Create: `crates/core/src/domain/prices.rs`
- Create: `crates/core/src/domain/errors.rs`
- Create: `crates/core/src/domain/mcp.rs`
- Create: `crates/core/src/connector.rs`
- Create: `crates/core/src/session/mod.rs`
- Create: `crates/core/src/session/cookies.rs`
- Create: `crates/core/src/session/solver.rs`
- Create: `crates/core/src/session/manager.rs`
- Create: `crates/core/src/transport/mod.rs`
- Create: `crates/core/src/transport/fingerprint.rs`
- Create: `crates/core/src/services/mod.rs`
- Create: `crates/core/src/services/aggregator.rs`
- Create: `crates/core/tests/core_tests.rs`

**Interfaces:**
- Produces:
  - `ru_marketplace_core::domain::{Marketplace, ProductOffer, PriceTier, Currency, StockStatus, CardType, SubscriptionType, WafProtection, ScraperError, SearchFilterOptions, SortOrder, McpTool, McpCallResult, JsonRpcRequest, JsonRpcResponse, error_codes}`
  - `ru_marketplace_core::connector::MarketplaceConnector` (async trait)
  - `ru_marketplace_core::session::{SessionSolver, SessionManager, CookieStore, SessionData, SessionStatusInfo}`
  - `ru_marketplace_core::transport::{apply_browser_fingerprint, build_browser_headers, DEFAULT_DESKTOP_UA, SEC_CH_UA}`
  - `ru_marketplace_core::services::{AggregatorService, CompareResult, SourceOutcome}`

- [ ] **Step 1: Write `crates/core/Cargo.toml`**
Declare dependencies using `workspace = true` (`futures`, `async-trait`, `reqwest`, `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`).

- [ ] **Step 2: Implement `SessionSolver` trait and `SessionStatusInfo`**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusInfo {
    pub marketplace: Marketplace,
    pub solver_active: bool,
    pub has_cached_cookies: bool,
    pub cookies_expired: bool,
    pub expires_at: Option<u64>,
    pub details: String,
}

#[async_trait]
pub trait SessionSolver: Send + Sync {
    fn marketplace(&self) -> Marketplace;
    async fn is_active(&self) -> bool;
    async fn start_session(&self) -> Result<String, ScraperError>;
    async fn stop_session(&self) -> Result<String, ScraperError>;
    async fn status(&self) -> SessionStatusInfo;
    async fn harvest(&self) -> Result<SessionData, ScraperError>;
    async fn invalidate(&self) -> Result<(), ScraperError>;
}
```

- [ ] **Step 3: Implement universal `SessionManager`**
Support `register_solver(Arc<dyn SessionSolver>)`, `start_session(target)`, `stop_session(target)`, `session_status(target)`, `refresh(marketplace)`.

- [ ] **Step 4: Port domain, connector trait, transport, and aggregator service**
Move models, prices, error types, browser headers, and aggregator service into `crates/core`.

- [ ] **Step 5: Run tests for `crates/core`**
Run: `cargo test -p ru-marketplace-core -j 2`
Expected: PASS.

---

### Task 3: Implement `crates/solver` (`ru-marketplace-solver`)

**Files:**
- Create: `crates/solver/Cargo.toml`
- Create: `crates/solver/src/lib.rs`
- Create: `crates/solver/src/native_solver.rs`
- Create: `crates/solver/src/cdp.rs`
- Create: `crates/solver/tests/solver_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core`
- Produces:
  - `ru_marketplace_solver::NativeCdpSolver` (implements `SessionSolver`)
  - Multi-tab support: `fetch_in_tab(marketplace: Marketplace, url: &str) -> Result<String, ScraperError>`
  - Tab management: Ozon tab (`https://www.ozon.ru/`) and WB tab (`https://www.wildberries.ru/`)
  - Process management: Xvfb `:99`, `PR_SET_PDEATHSIG`, 5-minute idle watchdog, graceful close

- [ ] **Step 1: Write `crates/solver/Cargo.toml`**
Declare `ru-marketplace-core`, `async-tungstenite`, `libc`, `tokio`, `serde`, `serde_json`, `tracing`, `async-trait`, `futures`. Dev-dependencies: `chromiumoxide`.

- [ ] **Step 2: Implement multi-tab CDP client and `NativeCdpSolver`**
Implement browser launch on Xvfb `:99` with standard web security (no `--disable-web-security`).
Support attached target sessions for Ozon and WB:
- Ozon session: navigates to `https://www.ozon.ru/`, solves Variti PoW, executes `fetch()` to `/api/composer-api.bx/...`.
- WB session: navigates to `https://www.wildberries.ru/`, executes `fetch()` to `https://search.wb.ru/exactmatch/...` with genuine Chrome TLS and WB's allowed origin.

- [ ] **Step 3: Implement `SessionSolver` for `NativeCdpSolver`**
Implement `is_active`, `start_session`, `stop_session`, `status`, `harvest`, `invalidate`.

- [ ] **Step 4: Run tests for `crates/solver`**
Run: `cargo test -p ru-marketplace-solver -j 2`
Expected: PASS.

---

### Task 4: Implement `crates/ozon` (`ru-marketplace-ozon`)

**Files:**
- Create: `crates/ozon/Cargo.toml`
- Create: `crates/ozon/src/lib.rs`
- Create: `crates/ozon/src/parser.rs`
- Create: `crates/ozon/src/connector.rs`
- Create: `crates/ozon/tests/ozon_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core`, `ru_marketplace_solver`
- Produces:
  - `ru_marketplace_ozon::OzonConnector`
  - `ru_marketplace_ozon::{parse_composer_json, parse_card_json, parse_price_str}`

- [ ] **Step 1: Write `crates/ozon/Cargo.toml`**
Declare `ru-marketplace-core`, `ru-marketplace-solver`, `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`, `async-trait`, `futures`.

- [ ] **Step 2: Implement Composer JSON parser in `crates/ozon/src/parser.rs`**
Port `parse_composer_json`, `parse_card_json`, `parse_price_str`, widget states, and security incident detection.

- [ ] **Step 3: Implement `OzonConnector` in `crates/ozon/src/connector.rs`**
Implement `MarketplaceConnector`. Search and card requests execute via `solver.fetch_in_tab(Marketplace::Ozon, &url_param)`.

- [ ] **Step 4: Run tests for `crates/ozon`**
Run: `cargo test -p ru-marketplace-ozon -j 2`
Expected: PASS.

---

### Task 5: Implement `crates/wb` (`ru-marketplace-wb`)

**Files:**
- Create: `crates/wb/Cargo.toml`
- Create: `crates/wb/src/lib.rs`
- Create: `crates/wb/src/basket.rs`
- Create: `crates/wb/src/connector.rs`
- Create: `crates/wb/src/parser.rs`
- Create: `crates/wb/tests/wb_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core`, `ru_marketplace_solver`
- Produces:
  - `ru_marketplace_wb::BasketResolver` (dynamic CDN shard resolver with caching)
  - `ru_marketplace_wb::WildberriesConnector`
  - `ru_marketplace_wb::parse_products_json`

- [ ] **Step 1: Write `crates/wb/Cargo.toml`**
Declare `ru-marketplace-core`, `ru-marketplace-solver`, `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`, `async-trait`, `futures`.

- [ ] **Step 2: Port `BasketResolver` into `crates/wb/src/basket.rs`**
Implement `calculate_vol`, `calculate_part`, concurrent CDN probing across `basket-01..40.wbbasket.ru`, and thread-safe caching.

- [ ] **Step 3: Implement `WildberriesConnector` in `crates/wb/src/connector.rs`**
- `search()`: executes via `solver.fetch_in_tab(Marketplace::Wildberries, &search_url)` (bypassing wbaas WAF 403).
- `get_card()`: executes via `basket_resolver.get_card(nm)` directly to CDN shards (fast-path 50ms, no browser needed).
- `get_reviews()` / `get_price_history()`: direct HTTP queries.

- [ ] **Step 4: Run tests for `crates/wb`**
Run: `cargo test -p ru-marketplace-wb -j 2`
Expected: PASS.

---

### Task 6: Implement `crates/dns` & `crates/citilink`

**Files:**
- Create: `crates/dns/Cargo.toml`
- Create: `crates/dns/src/lib.rs`
- Create: `crates/dns/src/connector.rs`
- Create: `crates/citilink/Cargo.toml`
- Create: `crates/citilink/src/lib.rs`
- Create: `crates/citilink/src/connector.rs`
- Create: `crates/dns/tests/dns_tests.rs`
- Create: `crates/citilink/tests/citilink_tests.rs`

**Interfaces:**
- Consumes: `ru_marketplace_core`, `ru_marketplace_solver`
- Produces:
  - `ru_marketplace_dns::DnsConnector`
  - `ru_marketplace_citilink::CitilinkConnector`

- [ ] **Step 1: Implement `crates/dns`**
Extract DNS search logic, `MarketplaceConnector`, and browser fallback on 401.

- [ ] **Step 2: Implement `crates/citilink`**
Extract Citilink search logic, `MarketplaceConnector`, and browser fallback on challenge.

- [ ] **Step 3: Run connector tests**
Run: `cargo test -p ru-marketplace-dns -p ru-marketplace-citilink -j 2`
Expected: PASS.

---

### Task 7: Implement `crates/mcp` (`ru-marketplace-mcp`)

**Files:**
- Create: `crates/mcp/Cargo.toml`
- Create: `crates/mcp/src/lib.rs`
- Create: `crates/mcp/src/server.rs`
- Create: `crates/mcp/src/main.rs`
- Create: `crates/mcp/tests/mcp_server_tests.rs`

**Interfaces:**
- Consumes: All workspace crates (`ru-marketplace-core`, `ru-marketplace-solver`, `ru-marketplace-ozon`, `ru-marketplace-wb`, `ru-marketplace-dns`, `ru-marketplace-citilink`)
- Produces:
  - `ru_marketplace_mcp::McpServer`
  - Binary `ru-marketplace-mcp`
  - Universal tools: `session_start`, `session_stop`, `session_status`
  - Aliases: `ozon_session_start`, `ozon_session_stop`

- [ ] **Step 1: Write `crates/mcp/Cargo.toml`**
Declare dependencies on all workspace crates, `clap`, `tokio`, `tracing`, `tracing-subscriber`, `serde`, `serde_json`.

- [ ] **Step 2: Implement tools schema and routing in `server.rs`**
Add `session_start`, `session_stop`, `session_status`. Support parameter `marketplace: "all" | "ozon" | "wb" | "dns" | "citilink"`.
Preserve `ozon_session_start` and `ozon_session_stop` aliases.

- [ ] **Step 3: Implement CLI and stdio transport in `main.rs`**
Wire `--compare`, `refresh-tokens`, `--stdio`. Register `NativeCdpSolver` into `SessionManager`. Ensure Stdio shutdown stops active sessions.

- [ ] **Step 4: Run tests for `crates/mcp`**
Run: `cargo test -p ru-marketplace-mcp -j 2`
Expected: PASS.

---

### Task 8: Cleanup, Verification & Release

**Files:**
- Remove: `src/` (superseded by `crates/*`)
- Remove: `tests/` (superseded by crate integration tests)
- Update: `dist/ru-marketplace-mcp`

- [ ] **Step 1: Remove legacy monolithic `src/` and `tests/`**
Delete old root directories after verifying crate tests.

- [ ] **Step 2: Run full workspace test suite**
Run: `cargo test --all -j 2`
Expected: All tests pass across the entire workspace.

- [ ] **Step 3: Run clippy and format checks**
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo fmt --all -- --check`

- [ ] **Step 4: Build release binary with Cranelift and test CLI**
Run: `cargo build --release -p ru-marketplace-mcp -j 2`
Test: `target/release/ru-marketplace-mcp --help`
Test: `target/release/ru-marketplace-mcp --compare "чайник" --limit 1`

- [ ] **Step 5: Update `dist/ru-marketplace-mcp`**
Copy `target/release/ru-marketplace-mcp` to `dist/ru-marketplace-mcp`.
Verify with Nix: `nix build .#default --no-link`
