use axum::{
    extract::{Request, State},
    http::request::Parts,
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
use crate::{
    database::models::auth::UserId,
    services::{auth::AuthError, notifications::{self, Notification}, shared::{env::get_env_variable, urls::parse_public_origin}},
};

#[derive(Clone)]
struct AgentIdentity {
    user_id: UserId,
    hash: String,
}
#[derive(Clone)]
struct TradingMcp {
    public_origin: Arc<str>,
}
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

fn identity(parts: &Parts) -> anyhow::Result<&AgentIdentity> {
    parts.extensions.get::<AgentIdentity>()
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

async fn present_proposal(user_id: &UserId, public_origin: &str, proposal: super::Proposal, notice: Notice) -> CallToolResult {
    let url = approval::approval_url(&approval::LinkInput {
        configured_origin: Some(public_origin),
        proposal_id: &proposal.id,
    });
    let telegram = match notice {
        Notice::New => match notifications::configured(user_id).await {
            Ok(true) => {
                let message = approval::proposal_notification(&proposal, url.as_deref());
                let user_id = user_id.clone();
                tokio::spawn(async move {
                    if let Err(error) = (Notification { content: message }).send_without_preview(&user_id).await {
                        tracing::warn!(%error, "Trading proposal Telegram notification failed");
                    }
                });
                Some("requested")
            }
            Ok(false) => Some("not_configured"),
            Err(error) => {
                tracing::warn!(%error, "Trading proposal Telegram configuration unavailable");
                Some("unavailable")
            }
        },
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
        result(async {
            let agent = identity(&parts)?;
            super::search(&agent.user_id, &agent.hash, &input.query).await
        }.await)
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
            let agent = identity(&parts)?;
            super::quotes::get(&agent.user_id, &agent.hash, &input.tickers, input.max_age_seconds.unwrap_or(1800)).await
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
        let agent = match identity(&parts) {
            Ok(agent) => agent,
            Err(error) => return tool_error(error),
        };
        match super::submit(&agent.user_id, &agent.hash, input).await {
            Ok(outcome) => present_proposal(&agent.user_id, &self.public_origin, outcome.proposal, if outcome.created { Notice::New } else { Notice::Replay }).await,
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
        let agent = match identity(&parts) {
            Ok(agent) => agent,
            Err(error) => return tool_error(error),
        };
        match super::get_proposal(&agent.user_id, &agent.hash, &input.id).await {
            Ok(proposal) => present_proposal(&agent.user_id, &self.public_origin, proposal, Notice::Read).await,
            Err(error) => tool_error(error),
        }
    }
}
async fn authenticate(State(public_origin): State<String>, mut request: Request, next: Next) -> Response {
    let headers = request.headers();
    if headers.contains_key(axum::http::header::AUTHORIZATION) && headers.get_all(axum::http::header::COOKIE).iter().any(|header| {
        header.to_str().ok().is_some_and(|cookies| cookies.split(';').any(|cookie| {
            cookie.trim().split_once('=').is_some_and(|(name, _)| name.trim() == "doughbox_session")
        }))
    }) {
        return AuthError::AmbiguousCredentials.into_response();
    }
    if let Some(origin) = headers.get(axum::http::header::ORIGIN) {
        if origin.to_str().ok().and_then(parse_public_origin).as_deref() != Some(public_origin.as_str()) {
            return AuthError::CsrfRejected.into_response();
        }
    }
    let token = headers.get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|token| token.starts_with("doughbox_trading_") && token.len() <= 128);
    let Some(token) = token else {
        return AuthError::Unauthenticated.into_response();
    };
    let hash = super::token_hash(token);
    let user_id = match super::authenticate(&hash).await {
        Ok(user_id) => user_id,
        Err(_) => return AuthError::Unauthenticated.into_response(),
    };
    request.extensions_mut().insert(AgentIdentity { user_id, hash });
    next.run(request).await
}

fn allowed_hosts(configured: Option<&str>) -> anyhow::Result<Vec<String>> {
    let hosts: Vec<String> = configured.unwrap_or("localhost,127.0.0.1,::1")
        .split(',').map(str::trim).filter(|host| !host.is_empty()).map(str::to_owned).collect();
    anyhow::ensure!(!hosts.is_empty(), "TRADING_MCP_HOSTS must contain at least one allowed host.");
    Ok(hosts)
}

pub fn router(public_origin: &str) -> anyhow::Result<Router> {
    let hosts = allowed_hosts(get_env_variable("TRADING_MCP_HOSTS").as_deref())?;
    let origin: Arc<str> = Arc::from(public_origin);
    let service: StreamableHttpService<TradingMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(TradingMcp { public_origin: origin.clone() }),
            Arc::default(),
            StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true)
                .with_allowed_hosts(hosts),
        );
    Ok(Router::new()
        .nest_service("/api/trading/mcp", service)
        .layer(middleware::from_fn_with_state(public_origin.to_owned(), authenticate)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_empty_host_allowlist_is_rejected() {
        for configured in ["", "  ", ", ,"] {
            assert!(allowed_hosts(Some(configured)).is_err());
        }
        assert_eq!(allowed_hosts(Some(" example.com, localhost ")).unwrap(), ["example.com", "localhost"]);
        assert!(!allowed_hosts(None).unwrap().is_empty());
    }

    async fn boundary(origin: Option<&str>, cookie: Option<&str>, bearer: Option<&str>) -> (axum::http::StatusCode, Value) {
        use axum::{body::{Body, to_bytes}, http::Request, routing::post};
        use tower::ServiceExt;
        let app = Router::new().route("/", post(|| async { "must not authorize" }))
            .layer(middleware::from_fn_with_state("https://doughbox.example".to_owned(), authenticate));
        let mut request = Request::builder().method("POST").uri("/");
        if let Some(origin) = origin { request = request.header("origin", origin); }
        if let Some(cookie) = cookie { request = request.header("cookie", cookie); }
        if let Some(bearer) = bearer { request = request.header("authorization", bearer); }
        let response = app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn mcp_origin_is_configured_and_missing_origin_does_not_grant_authority() {
        use axum::http::StatusCode;
        for origin in [None, Some("https://DOUGHBOX.example:443/")] {
            let (status, body) = boundary(origin, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["error"], "Unauthenticated");
        }
        for origin in ["https://other.example", "null", "https://doughbox.example/private"] {
            let (status, body) = boundary(Some(origin), None, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["error"], "CsrfRejected");
        }
    }

    #[tokio::test]
    async fn mcp_rejects_mixed_cookie_and_bearer_before_token_resolution() {
        use axum::http::StatusCode;
        let (status, body) = boundary(None, Some("theme=dark; doughbox_session=opaque"), Some("Bearer doughbox_trading_fixture")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "AmbiguousCredentials");
        let (status, body) = boundary(None, Some("doughbox_session=opaque"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "Unauthenticated");
    }
}
