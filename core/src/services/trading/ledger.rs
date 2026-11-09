use anyhow::{ensure, Context};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    database::{db_client, models::{auth::UserId, trade::Trade}, queries::{trade::get_trades_page, QueryFilter}},
    services::events::{get_events_page, EventPageFilter, EventType, PortfolioEvent},
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TradeReadInput {
    /// Inclusive UTC date, YYYY-MM-DD. Omit for the 30 calendar days ending on until_date.
    pub from_date: Option<String>,
    /// Inclusive UTC date, YYYY-MM-DD. Defaults to today.
    pub until_date: Option<String>,
    /// Current canonical ISIN, including recorded trades under an older identifier.
    pub isin: Option<String>,
    /// Exact recorded broker name.
    pub broker: Option<String>,
    /// Buy or Sell, matching the recorded transaction direction.
    pub direction: Option<Direction>,
    /// Page size, 1–100; default 50.
    pub limit: Option<i64>,
    /// Nonnegative offset into matching rows; default 0. Keep filters unchanged between pages.
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(super) enum Direction {
    Buy,
    Sell,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ActivityReadInput {
    /// Inclusive UTC date, YYYY-MM-DD. Omit for the 30 calendar days ending on until_date.
    pub from_date: Option<String>,
    /// Inclusive UTC date, YYYY-MM-DD. Defaults to today.
    pub until_date: Option<String>,
    /// Exact event identifier: canonical ISIN, manual asset ID, FX pair (e.g. EURUSD), or fund-report ID.
    pub identifier: Option<String>,
    /// Exact recorded broker name.
    pub broker: Option<String>,
    /// CashInterest, ShareInterest, PrivateDebtInterest, Dividend, Trade, FxConversion, DividendAequivalent, Deposit, Withdrawal, PrincipalAdvance, PrincipalRepayment, OpeningBalance, BalanceReconciliation, or Valuation.
    pub event_type: Option<String>,
    /// Page size, 1–100; default 50.
    pub limit: Option<i64>,
    /// Nonnegative offset into matching rows; default 0. Keep filters unchanged between pages.
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub(super) struct LedgerPage<T> {
    generated_at: i64,
    from_date: NaiveDate,
    until_date: NaiveDate,
    limit: i64,
    offset: i64,
    next_offset: Option<i64>,
    items: Vec<T>,
    notice: &'static str,
}

struct Window {
    from_date: NaiveDate,
    until_date: NaiveDate,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: i64,
    offset: i64,
}

impl Window {
    fn parse(from: Option<&str>, until: Option<&str>, limit: Option<i64>, offset: Option<i64>, today: NaiveDate) -> anyhow::Result<Self> {
        fn date(value: &str) -> anyhow::Result<NaiveDate> {
            ensure!(value.len() == 10, "Use UTC dates in YYYY-MM-DD format.");
            NaiveDate::parse_from_str(value, "%Y-%m-%d").context("Use valid UTC dates in YYYY-MM-DD format.")
        }
        let until_date = until.map(date).transpose()?.unwrap_or(today);
        let from_date = match from {
            Some(from) => date(from)?,
            None => until_date.checked_sub_signed(Duration::days(29)).context("Date range is out of bounds.")?,
        };
        ensure!(from_date <= until_date, "from_date must not be after until_date.");
        let start = from_date.and_hms_opt(0, 0, 0).context("Date range is out of bounds.")?.and_utc();
        // Half-open SQL boundaries include the final day's fractional seconds.
        let end = until_date.succ_opt().and_then(|day| day.and_hms_opt(0, 0, 0)).context("Date range is out of bounds.")?.and_utc();
        let limit = limit.unwrap_or(50);
        let offset = offset.unwrap_or(0);
        ensure!((1..=100).contains(&limit), "limit must be between 1 and 100.");
        ensure!(offset >= 0 && offset.checked_add(limit).is_some(), "offset must be nonnegative and leave room for the next page.");
        Ok(Self { from_date, until_date, start, end, limit, offset })
    }

    fn page<T>(self, mut items: Vec<T>, notice: &'static str) -> LedgerPage<T> {
        let has_more = items.len() > self.limit as usize;
        items.truncate(self.limit as usize);
        LedgerPage {
            generated_at: Utc::now().timestamp(),
            from_date: self.from_date,
            until_date: self.until_date,
            limit: self.limit,
            offset: self.offset,
            next_offset: has_more.then_some(self.offset + self.limit),
            items,
            notice,
        }
    }
}

pub(super) async fn trades(user_id: &UserId, hash: &str, input: TradeReadInput) -> anyhow::Result<LedgerPage<Trade>> {
    let window = Window::parse(input.from_date.as_deref(), input.until_date.as_deref(), input.limit, input.offset, Utc::now().date_naive())?;
    let filter = QueryFilter {
        isin: input.isin,
        broker: input.broker,
        direction: input.direction.map(|direction| match direction { Direction::Buy => "Buy", Direction::Sell => "Sell" }.into()),
        from_date: Some(window.start),
        until_date: Some(window.end),
        limit: None,
    };
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    super::store::agent_connection(user_id, &tx, hash).await?;
    let items = get_trades_page(user_id, &filter, window.limit + 1, window.offset).await?;
    tx.commit().await?;
    Ok(window.page(items, "Recorded imported security trades only, not trade proposals, broker order status or proof of broker synchronization. Quantities and prices are recorded transaction values, not today's split-adjusted holdings. Prices use each trade's currency; eur_avg_price_per_unit and fees are EUR. Keep returned date bounds and filters unchanged when following next_offset. Ledger edits or backdated imports between requests can shift offset pages."))
}

pub(super) async fn activity(user_id: &UserId, hash: &str, input: ActivityReadInput) -> anyhow::Result<LedgerPage<PortfolioEvent>> {
    let window = Window::parse(input.from_date.as_deref(), input.until_date.as_deref(), input.limit, input.offset, Utc::now().date_naive())?;
    let event_type = input.event_type.map(|value| serde_json::from_value::<EventType>(serde_json::Value::String(value)).context("Unknown event_type; use one of the documented timeline event types.")).transpose()?;
    let filter = EventPageFilter {
        from_date: window.start,
        until_date: window.end,
        isin: input.identifier,
        broker: input.broker,
        event_type,
        limit: window.limit + 1,
        offset: window.offset,
    };
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    super::store::agent_connection(user_id, &tx, hash).await?;
    let items = get_events_page(user_id, &filter).await?;
    tx.commit().await?;
    Ok(window.page(items, "Recorded Doughbox timeline, not live broker activity or order status. Event types distinguish transactions, income, balances, valuations and fund tax reports; they must not all be summed as cash flows or returns. Security trade units/prices use the timeline's split adjustment; get_trades preserves recorded transaction quantities. Read currency, total_currency, direction and unit_label for each event. Keep returned date bounds and filters unchanged when following next_offset. Ledger edits or backdated imports between requests can shift offset pages."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inclusive_until_date_covers_fractional_seconds_and_leap_day() {
        let today = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let window = Window::parse(Some("2024-02-29"), Some("2024-02-29"), None, None, today).unwrap();
        assert_eq!(window.start.to_rfc3339(), "2024-02-29T00:00:00+00:00");
        assert_eq!(window.end.to_rfc3339(), "2024-03-01T00:00:00+00:00");
    }

    #[test]
    fn malformed_reversed_dates_and_unsafe_pagination_are_rejected() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        for date in ["2026-02-30", "2026-1-01", "yesterday", "2026-11-09T00:00:00Z"] {
            assert!(Window::parse(Some(date), None, None, None, today).is_err());
        }
        assert!(Window::parse(Some("2026-11-10"), Some("2026-11-09"), None, None, today).is_err());
        for limit in [0, -1, 101] {
            assert!(Window::parse(None, None, Some(limit), None, today).is_err());
        }
        for offset in [-1, i64::MAX] {
            assert!(Window::parse(None, None, None, Some(offset), today).is_err());
        }
    }
}
