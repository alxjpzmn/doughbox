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

use super::{approval, SubmitInput, pies::{self, PieAccess, PieRateLimited, SubmitPieInput}};
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PieInput {
    /// Positive int64 broker Pie ID, encoded as a decimal string, never a JSON number.
    pie_id: String,
}

#[derive(Serialize)]
struct AgentPortfolio {
    currency: &'static str,
    notice: &'static str,
    #[serde(flatten)]
    overview: crate::services::portfolio::PortfolioOverview,
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
        Err(error) => tool_error(error),
    }
}
fn tool_error(error: anyhow::Error) -> CallToolResult {
    if let Some(rate) = error.downcast_ref::<PieRateLimited>() {
        return CallToolResult::structured_error(json!({
            "code":"TradingRateLimited", "message":rate.to_string(),
            "retry_after_seconds":rate.retry_after_seconds,
            "details":{"retry_after_seconds":rate.retry_after_seconds}
        }));
    }
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
    value["approval_note"] = json!("Opening approval_url does not approve or dispatch anything. The signed-in owner must confirm on the Trading page.");
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
        description = "Read the authenticated owner's current recorded Doughbox portfolio across all brokers, including securities, cash accounts, gold, property and private debt. No arguments. Returns generated_at (Unix seconds), EUR total_value, positions with exact units or native-currency balances (unit_label), EUR value and percentage share, plus valuation/return completeness fields. Missing values remain null; totals and shares cover known values only. Recomputed from recorded activity and stored prices, NOT a weekly snapshot, live broker positions, Trading 212 availableToTrade, or executable market prices. generated_at is calculation time, not price freshness or proof of broker synchronization. Does not refresh prices, import transactions or authorize approval."
    )]
    async fn get_portfolio(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            let mut client = crate::database::db_client().await?;
            let tx = client.transaction().await?;
            // Serialize token use with rotation, disconnect and account revocation.
            super::store::agent_connection(&agent.user_id, &tx, &agent.hash).await?;
            let overview = crate::services::portfolio::get_portfolio_overview(&agent.user_id).await?;
            tx.commit().await?;
            Ok(AgentPortfolio {
                currency: "EUR",
                notice: "Current recorded Doughbox holdings, not live broker balances. Values use stored prices and FX; generated_at is calculation time, not price freshness. Units reflect recorded activity only. Missing values are null; total_value and shares include known values only. Cash units are native-currency balances identified by unit_label.",
                overview,
            })
        }.await)
    }
    #[tool(
        description = "Read recorded imported security trades across the authenticated owner's brokers, NOT proposals or live broker order status. Optional inclusive UTC from_date/until_date (YYYY-MM-DD); default last 30 calendar days through today. Filter by canonical isin, exact broker and direction Buy/Sell. limit defaults to 50, maximum 100; offset defaults to 0. Returns items newest first, generated_at, resolved date bounds, limit, offset and nullable next_offset. Reuse resolved bounds and filters for subsequent pages; concurrent ledger edits can shift offset pages. Decimal-string quantities/prices preserve recorded transaction values, not today's split-adjusted units. Each trade has native currency, EUR price and EUR fees. Reads PostgreSQL only; never imports, refreshes, executes or approves."
    )]
    async fn get_trades(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<super::ledger::TradeReadInput>,
    ) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            super::ledger::trades(&agent.user_id, &agent.hash, input).await
        }.await)
    }
    #[tool(
        description = "Read the authenticated owner's recorded unified activity, NOT live broker executions or proposal status. Includes security/manual trades, dividends, interest, FX, deposits/withdrawals, loan principal, balance snapshots, manual valuations and owned-instrument fund tax reports. Optional inclusive UTC from_date/until_date (YYYY-MM-DD); default last 30 calendar days through today. Filter by exact identifier, broker and event_type. limit defaults to 50, maximum 100; offset defaults to 0. Returns items newest first with explicit event_type, currencies, direction, units and unit_label, plus resolved date bounds and nullable next_offset. Reuse resolved bounds and filters when paging; edits can shift offset pages. Timeline trade units/prices are split-adjusted; use get_trades for recorded transaction quantities. Balances, valuations and tax reports are not income or cash flows. Reads PostgreSQL only; never imports, refreshes, executes or approves."
    )]
    async fn get_activity(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<super::ledger::ActivityReadInput>,
    ) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            super::ledger::activity(&agent.user_id, &agent.hash, input).await
        }.await)
    }
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
    #[tool(description = "Read the connected Trading 212 account's live cash and positions before proposing trades. available_to_trade excludes pie cash and cash reserved for orders and is not a Doughbox cash account. quantity_available_for_trading is the only sellable share evidence; recorded holdings and shares inside Pies are not availability. Missing fields stay null. Respect TradingRateLimited retry_after_seconds. No mutation.")]
    async fn get_broker_account(&self, Extension(parts): Extension<Parts>) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            super::account::load(&agent.user_id, pies::PieAccess::Agent(&agent.hash), false).await
        }.await)
    }
    #[tool(
        description = "Propose one trade or a collection of up to 50 limit trades for HUMAN review. This NEVER executes or approves orders. Before saving, Doughbox reads live Trading 212 cash and positions. Sells require quantityAvailableForTrading at least as large as requested. Every buy requires present, strictly positive availableToTrade. Same-currency buys cannot exceed availableToTrade. Cross-currency buys are not treated as funded because Doughbox does not convert currencies. Pie cash and recorded Doughbox holdings are not buying power or sellable shares. Use exact Trading 212 tickers, positive decimal strings and an explicit buy/sell side. Reuse client_request_id only for identical intent. The result includes approval_url; opening it does not approve or submit the order."
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
    #[tool(description = "List native Trading 212 Pies for this connection. Live broker configuration, not imported Doughbox holdings. No arguments and no mutation. List names may be unavailable; use get_pie for selected IDs. Explicitly respect TradingRateLimited retry_after_seconds; do not poll.")]
    async fn get_pies(&self, Extension(parts): Extension<Parts>) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            pies::list(&agent.user_id, PieAccess::Agent(&agent.hash)).await
        }.await)
    }

    #[tool(description = "Read one native Trading 212 Pie by its exact positive decimal-string pie_id. Reports configured targets separately from current weights and owned quantities; missing evidence remains null. No mutation. Respect TradingRateLimited retry_after_seconds.")]
    async fn get_pie(&self, Extension(parts): Extension<Parts>, Parameters(input): Parameters<PieInput>) -> CallToolResult {
        result(async {
            let agent = identity(&parts)?;
            pies::detail(&agent.user_id, PieAccess::Agent(&agent.hash), &input.pie_id).await
        }.await)
    }

    #[tool(description = "Propose native Pie creation, settings/allocation update, composition duplication or permanent deletion for the signed-in owner's explicit review. NEVER approves, executes, funds or trades shares. Weights are exact fractional decimal strings (1–50 catalog STOCK/ETF tickers, at most four decimals, sum exactly 1); updates replace the whole allocation map. Omitted settings preserve the saved snapshot; null clearing is unsupported. Trading 212 rejects an update whose name equals the live name, HTTP 400 'Check the id and rename the pie if you want to apply changes', even after get_pie succeeds. On every update, read the current name and set changes.name to the next version for owner review: if it ends with space, v, and a positive integer, increment that integer ('Growth v2' → 'Growth v3'); otherwise append ' v1' ('Growth' → 'Growth v1'). Never send the current name, a temporary suffix, or a plan to rename it back. Doughbox will not invent the version. Duplicate copies composition only, not holdings, AutoInvest or goals. Delete requires empty holdings and a separate browser-owner Pie cash acknowledgement. Reuse client_request_id only for identical intent; unknown dispatch outcomes must be checked manually, never resubmitted. Show the returned approval_url unchanged.")]
    async fn submit_pie(&self, Extension(parts): Extension<Parts>, Parameters(input): Parameters<SubmitPieInput>) -> CallToolResult {
        let agent = match identity(&parts) {
            Ok(agent) => agent,
            Err(error) => return tool_error(error),
        };
        match pies::submit(&agent.user_id, PieAccess::Agent(&agent.hash), input).await {
            Ok(outcome) => present_proposal(&agent.user_id, &self.public_origin, outcome.proposal, if outcome.created { Notice::New } else { Notice::Replay }).await,
            Err(error) => tool_error(error),
        }
    }
    #[tool(
        description = "Read a previously submitted trade or native Pie proposal and its review/dispatch/evidence status. Unknown outcomes must be checked in Trading 212; never resubmit them. Pie settings confirmation is not a fill or funding. No approval capability. Pending results include approval_url; show that exact URL to the user. Opening it does not approve or dispatch anything."
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
