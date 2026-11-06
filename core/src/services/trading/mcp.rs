use axum::{
    extract::Request,
    http::{request::Parts, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use rmcp::{
    handler::server::{tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use super::{approval, SubmitInput};
use crate::services::notifications::{self, Notification};

#[derive(Clone)]
struct AgentToken(String);
#[derive(Clone)]
struct TradingMcp;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuoteInput {
    /// One to fifty exact Trading 212 tickers from search_instruments.
    tickers: Vec<String>,
    /// Stale/selection age, default 1800 seconds; never extends 168-hour source-event expiry.
    #[serde(default)]
    max_age_seconds: Option<i32>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProposalInput {
    id: String,
}

fn hash(parts: &Parts) -> anyhow::Result<&str> {
    parts
        .extensions
        .get::<AgentToken>()
        .map(|token| token.0.as_str())
        .ok_or_else(|| anyhow::anyhow!("A trading agent token is required on every request."))
}
fn result<T: Serialize>(result: anyhow::Result<T>) -> CallToolResult {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => CallToolResult::structured(value),
            Err(_) => CallToolResult::structured_error(
                json!({"message":"Could not encode trading result."}),
            ),
        },
        Err(error) => CallToolResult::structured_error(json!({"message": error.to_string()})),
    }
}
fn tool_error(error: anyhow::Error) -> CallToolResult {
    CallToolResult::structured_error(json!({"message": error.to_string()}))
}
enum Notice {
    New,
    Replay,
    Read,
}

fn present_proposal(parts: &Parts, proposal: super::Proposal, notice: Notice) -> CallToolResult {
    let configured = crate::services::shared::env::get_env_variable("DOUGHBOX_PUBLIC_URL");
    let allowed = crate::services::shared::env::get_env_variable("TRADING_MCP_HOSTS")
        .unwrap_or_else(|| "localhost,127.0.0.1,::1".into());
    let host = request_host(parts);
    let url = approval::approval_url(&approval::LinkInput {
        configured_origin: configured.as_deref(),
        allowed_hosts: &allowed,
        host: host.as_deref(),
        uri_scheme: parts.uri.scheme_str(),
        forwarded_proto: parts
            .headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok()),
        proposal_id: &proposal.id,
    });
    let telegram = match notice {
        Notice::New if notifications::configured() => {
            let message = approval::proposal_notification(&proposal, url.as_deref());
            tokio::spawn(async move {
                if let Err(error) = (Notification { content: message }).send_without_preview().await {
                    tracing::warn!(%error, "Trading proposal Telegram notification failed");
                }
            });
            Some("requested")
        }
        Notice::New => Some("not_configured"),
        Notice::Replay => Some("not_repeated"),
        Notice::Read => None,
    };
    let text = approval::proposal_review_text(&proposal, url.as_deref(), telegram);
    let mut value = match serde_json::to_value(&proposal) {
        Ok(value) => value,
        Err(_) => {
            return CallToolResult::structured_error(json!({"message":"Could not encode trading result."}));
        }
    };
    match &url {
        Some(url) => value["approval_url"] = Value::String(url.clone()),
        None => value["approval_url"] = Value::Null,
    }
    value["approval_note"] = json!("Opening approval_url does not approve or submit the order. The signed-in owner must confirm on the Trading page.");
    if let Some(telegram) = telegram {
        value["telegram_notification"] = json!(telegram);
    }
    let mut result = CallToolResult::structured(value);
    result.content.insert(0, ContentBlock::text(text));
    result
}

fn request_host(parts: &Parts) -> Option<String> {
    if let Some(host) = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        let host = host.trim();
        if !host.is_empty() {
            return Some(host.to_owned());
        }
    }
    parts.uri.authority().map(|authority| authority.as_str().to_owned())
}
#[tool_router(server_handler)]
impl TradingMcp {
    #[tool(
        description = "Search the connected Trading 212 instrument catalog by ticker, name, or ISIN. Returns exact ticker, ISIN, and order currency. get_quotes reads stored delayed bulk post-trade references only, without exchange quote HTTP, price jobs or FX imports. Older broker catalogs may be backfilled from metadata. Neither search nor quotes chooses, submits or reprices an order. Limit prices MUST use the ticker's order currency; GBX means pence, not GBP."
    )]
    async fn search_instruments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<SearchInput>,
    ) -> CallToolResult {
        result(async { super::search(hash(&parts)?, &input.query).await }.await)
    }
    #[tool(
        description = "Get DELAYED last-trade price references for 1 to 50 exact Trading 212 tickers using only already-ingested bulk post-trade files. Resolves ISIN and order currency from the broker catalog, with metadata backfill for older catalogs. No exchange quote HTTP, price-job enqueue or FX import occurs on quote reads. Returns a nullable decimal-string price in order currency and a native reference with price, quantity, currency, feed, venue, source-event as_of and received_at. This is NOT a bid/ask, midpoint, live, executable or NBBO quote. Zero reported quantity is indicative; unknown quantity remains null. Selection prefers non-indicative, then within max_age_seconds (positive; default 1800), then requested currency/exact GBP-GBX units, then EUR, latest event and deterministic feed tie. Check age_seconds, stale, indicative and unavailable_reason. Prices expire 168 hours after source events; receipt and requested freshness never extend expiry. Stored dated ECB observations or exact GBP/GBX units convert prices; missing FX leaves price null but preserves native provenance. Coverage depends on completed imports across Xetra, Tradegate, Frankfurt, Cboe and Euronext, including unheld instruments; inactive schedules or feed failures mean stale/missing data. Choose explicit ticker-currency limit prices for human review; lookup never chooses or reprices orders."
    )]
    async fn get_quotes(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<QuoteInput>,
    ) -> CallToolResult {
        result(async {
            let hash = hash(&parts)?;
            super::quotes::get(hash, &input.tickers, input.max_age_seconds.unwrap_or(1800)).await
        }.await)
    }
    #[tool(
        description = "Propose one trade or a collection of up to 50 limit trades for HUMAN review in Doughbox's Trading tab. This NEVER executes or approves orders. Use exact Trading 212 tickers from search_instruments, positive decimal strings and an explicit buy/sell side. Fractional quantities are subject to Trading 212's instrument-price/position precision rules: do not assume four decimals or unlimited precision. Published examples allow two quantity decimals at prices 1 to below 10 and three at 10 to below 100; broker rules and full-position exceptions remain authoritative. Choose quantities explicitly before human review; never round an approved trade or automatically retry a rejected batch. get_quotes reads delayed bulk-ingested last-trade references only, without exchange quote HTTP, price jobs or FX imports. Check native feed/venue, event age, stale, indicative and unavailable_reason: a last trade is not live or executable. Choose explicit limit_price values in each ticker's order currency; no tool chooses, copies a reference into, or reprices a limit. Reuse client_request_id only with identical input. Batches execute sequentially, are NOT atomic, and stop on failure or uncertainty. Submitted means accepted by broker, not filled. No retries of uncertain outcomes, market orders, self-approval or policy bypass. The result includes approval_url. Show that exact URL to the user; opening it only opens the signed-in Trading page and does not approve or submit the order. Do not present any other URL from the title or note. A newly saved proposal also requests a Telegram notification with the same link when Telegram is configured."
    )]
    async fn submit_trades(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<SubmitInput>,
    ) -> CallToolResult {
        let hash = match hash(&parts) {
            Ok(hash) => hash,
            Err(error) => return tool_error(error),
        };
        match super::submit(hash, input).await {
            Ok(outcome) => present_proposal(&parts, outcome.proposal, if outcome.created { Notice::New } else { Notice::Replay }),
            Err(error) => tool_error(error),
        }
    }
    #[tool(
        description = "Read a previously submitted proposal and each trade's review/execution status. Unknown outcomes must be checked in Trading 212; do not resubmit them. No approval capability. Pending results include approval_url; show that exact URL to the user. Opening it does not approve or submit the order."
    )]
    async fn get_proposal(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<ProposalInput>,
    ) -> CallToolResult {
        let hash = match hash(&parts) {
            Ok(hash) => hash,
            Err(error) => return tool_error(error),
        };
        match super::get_proposal(hash, &input.id).await {
            Ok(proposal) => present_proposal(&parts, proposal, Notice::Read),
            Err(error) => tool_error(error),
        }
    }
}
async fn authenticate(mut request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) =
        token.filter(|token| token.starts_with("doughbox_trading_") && token.len() <= 128)
    else {
        return (
            StatusCode::UNAUTHORIZED,
            "A submission-only Trading agent Bearer token is required.",
        )
            .into_response();
    };
    let hash = super::token_hash(token);
    if super::authenticate(&hash).await.is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            "Invalid, revoked, or unavailable trading agent token.",
        )
            .into_response();
    }
    request.extensions_mut().insert(AgentToken(hash));
    next.run(request).await
}
pub fn router() -> Router {
    let hosts = crate::services::shared::env::get_env_variable("TRADING_MCP_HOSTS")
        .unwrap_or_else(|| "localhost,127.0.0.1,::1".into());
    let hosts: Vec<String> = hosts
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let service: StreamableHttpService<TradingMcp, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(TradingMcp),
            Arc::default(),
            StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true)
                .with_allowed_hosts(hosts),
        );
    Router::new()
        .nest_service("/api/trading/mcp", service)
        .layer(middleware::from_fn(authenticate))
}
