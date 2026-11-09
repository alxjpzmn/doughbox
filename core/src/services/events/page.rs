use super::*;

/// Filters apply to the event's displayed identifier and broker before pagination.
/// `until_date` is exclusive; callers supply a positive fetch limit and nonnegative offset.
#[derive(Debug, Clone)]
pub(crate) struct EventPageFilter {
    pub from_date: DateTime<Utc>,
    pub until_date: DateTime<Utc>,
    pub isin: Option<String>,
    pub broker: Option<String>,
    pub event_type: Option<EventType>,
    pub limit: i64,
    pub offset: i64,
}

// Source numbers are explicit tie breakers, not event IDs: different private
// tables may legitimately contain the same ID. Only the selected IDs leave SQL.
const PAGE_CANDIDATES: &str = r#"
WITH canonical AS (
    SELECT * FROM unnest($9::text[], $10::text[]) AS mapping(old_isin, new_isin)
), candidates AS (
    SELECT i.date, 0 AS source, i.id AS source_id, i.asset_id AS identifier,
           COALESCE(i.broker, 'Manual') AS broker,
           CASE i.principal WHEN 'Cash' THEN 'CashInterest'
                WHEN 'PrivateDebt' THEN 'PrivateDebtInterest' ELSE 'ShareInterest' END AS event_type
    FROM interest i WHERE i.user_id = $1
    UNION ALL
    SELECT r.date, 1, r.id::text, r.id::text, 'OeKB Fund Report', 'DividendAequivalent'
    FROM fund_report_oekb r
    LEFT JOIN canonical rc ON rc.old_isin = r.isin
    WHERE EXISTS (
        SELECT 1 FROM trade owned
        LEFT JOIN canonical oc ON oc.old_isin = owned.isin
        WHERE owned.user_id = $1
          AND COALESCE(oc.new_isin, owned.isin) = COALESCE(rc.new_isin, r.isin)
    )
    UNION ALL
    SELECT d.date, 2, d.id, COALESCE(c.new_isin, d.isin), d.broker, 'Dividend'
    FROM dividend d LEFT JOIN canonical c ON c.old_isin = d.isin WHERE d.user_id = $1
    UNION ALL
    SELECT t.date, 3, t.hash, COALESCE(c.new_isin, t.isin), t.broker, 'Trade'
    FROM trade t LEFT JOIN canonical c ON c.old_isin = t.isin WHERE t.user_id = $1
    UNION ALL
    SELECT f.date, 4, f.id, f.from_currency || f.to_currency, f.broker, 'FxConversion'
    FROM fx_conversion f WHERE f.user_id = $1
    UNION ALL
    SELECT t.date, 5, t.id, a.id, t.broker, 'Trade'
    FROM asset_trade t JOIN asset a ON a.user_id = t.user_id AND a.id = t.asset_id
    WHERE t.user_id = $1
    UNION ALL
    SELECT t.date, 6, t.id, a.id, 'Manual', t.kind
    FROM asset_transaction t JOIN asset a ON a.user_id = t.user_id AND a.id = t.asset_id
    WHERE t.user_id = $1
    UNION ALL
    SELECT s.date, 7, s.id, a.id, 'Manual',
           CASE WHEN s.kind = 'Opening' THEN 'OpeningBalance' ELSE 'BalanceReconciliation' END
    FROM asset_balance_snapshot s JOIN asset a ON a.user_id = s.user_id AND a.id = s.asset_id
    WHERE s.user_id = $1
    UNION ALL
    SELECT v.date, 8, v.id, a.id, 'Manual', 'Valuation'
    FROM asset_valuation v JOIN asset a ON a.user_id = v.user_id AND a.id = v.asset_id
    WHERE v.user_id = $1 AND v.source = 'Manual'
)
SELECT source, source_id FROM candidates
WHERE date >= $2 AND date < $3
  AND ($4::text IS NULL OR identifier = $4)
  AND ($5::text IS NULL OR broker = $5)
  AND ($6::text IS NULL OR event_type = $6)
ORDER BY date DESC, source ASC, source_id COLLATE "C" ASC
LIMIT $7 OFFSET $8
"#;

async fn query_page_candidates(
    user_id: &UserId,
    client: &Client,
    filter: &EventPageFilter,
    listing_changes: &[ListingChange],
) -> anyhow::Result<Vec<(i32, String)>> {
    // Match get_changed_identifier's first, one-step mapping, including when
    // the target itself has another listing change. Never recursively resolve.
    let mut seen = HashSet::new();
    let mut old_isins = Vec::new();
    let mut new_isins = Vec::new();
    for change in listing_changes {
        if seen.insert(change.from_identifier.as_str()) {
            old_isins.push(change.from_identifier.as_str());
            new_isins.push(change.to_identifier.as_str());
        }
    }
    let event_type = filter.event_type.as_ref().map(|kind| match kind {
        EventType::CashInterest => "CashInterest",
        EventType::ShareInterest => "ShareInterest",
        EventType::PrivateDebtInterest => "PrivateDebtInterest",
        EventType::Dividend => "Dividend",
        EventType::Trade => "Trade",
        EventType::FxConversion => "FxConversion",
        EventType::DividendAequivalent => "DividendAequivalent",
        EventType::Deposit => "Deposit",
        EventType::Withdrawal => "Withdrawal",
        EventType::PrincipalAdvance => "PrincipalAdvance",
        EventType::PrincipalRepayment => "PrincipalRepayment",
        EventType::OpeningBalance => "OpeningBalance",
        EventType::BalanceReconciliation => "BalanceReconciliation",
        EventType::Valuation => "Valuation",
    });
    client.query(PAGE_CANDIDATES, &[
        &user_id.as_str(), &filter.from_date, &filter.until_date,
        &filter.isin, &filter.broker, &event_type, &filter.limit, &filter.offset,
        &old_isins, &new_isins,
    ]).await?.into_iter().map(|row| {
        Ok((row.try_get("source")?, row.try_get("source_id")?))
    }).collect()
}

/// Read a globally ordered ledger page without importing, refreshing or mutating.
/// The SQL limit bounds both row retrieval and subsequent financial enrichment.
pub(crate) async fn get_events_page(
    user_id: &UserId,
    filter: &EventPageFilter,
) -> anyhow::Result<Vec<PortfolioEvent>> {
    let client = db_client().await?;
    let listing_changes = get_listing_changes().await?;
    let candidates = query_page_candidates(user_id, &client, filter, &listing_changes).await?;
    let positions: HashMap<_, _> = candidates.iter().enumerate()
        .map(|(position, (source, id))| ((*source, id.as_str()), position)).collect();
    let mut ordered_events = Vec::with_capacity(candidates.len());
    for source in 0..9 {
        let ids: Vec<&str> = candidates.iter()
            .filter(|(candidate_source, _)| *candidate_source == source)
            .map(|(_, id)| id.as_str()).collect();
        if ids.is_empty() {
            continue;
        }
        let rows = query_selected_rows(user_id, &client, source, &ids).await?;
        let row_positions = rows.iter().map(|row| {
            let id: &str = row.try_get("page_source_id")?;
            positions.get(&(source, id)).copied()
                .context("Selected event did not belong to the requested page")
        }).collect::<anyhow::Result<Vec<_>>>()?;
        let events = match source {
            0 => process_interest_rows(rows, Some(&client)).await?,
            1 => process_fund_report_rows(rows)?,
            2 => process_dividend_rows(user_id, rows, Some(&client)).await?,
            3 => process_trade_rows(user_id, rows, Some(&client)).await?,
            4 => process_fx_conversion_rows(rows)?,
            5 => process_asset_trade_rows(rows)?,
            6 => process_asset_transaction_rows(rows)?,
            7 => process_asset_snapshot_rows(rows)?,
            8 => process_asset_valuation_rows(rows)?,
            _ => unreachable!("Event page sources are defined above"),
        };
        ordered_events.extend(row_positions.into_iter().zip(events));
    }
    ordered_events.sort_unstable_by_key(|(position, _)| *position);
    Ok(ordered_events.into_iter().map(|(_, event)| event).collect())
}

async fn query_selected_rows(
    user_id: &UserId,
    client: &Client,
    source: i32,
    ids: &[&str],
) -> anyhow::Result<Vec<Row>> {
    let sql = match source {
        0 => "SELECT i.date, i.amount, i.currency, i.principal, i.withholding_tax, \
              i.withholding_tax_currency, i.amount_eur, i.broker, i.asset_id, \
              i.tax_treatment, a.name, a.unit_label, i.id, i.id AS page_source_id \
              FROM interest i LEFT JOIN asset a ON a.user_id = i.user_id AND a.id = i.asset_id \
              WHERE i.user_id = $1 AND i.id = ANY($2)",
        1 => {
            // Reports have numeric upstream IDs. Keep their indexed comparison
            // numeric; their displayed identifier and candidate key remain text.
            let report_ids = ids.iter().map(|id| id.parse::<i32>())
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(client.query(
                "SELECT date, id, currency, isin, id::text AS page_source_id \
                 FROM fund_report_oekb WHERE id = ANY($1)",
                &[&report_ids],
            ).await?);
        }
        2 => "SELECT date, amount, currency, isin, withholding_tax, withholding_tax_currency, \
              amount_eur, broker, id, id AS page_source_id FROM dividend \
              WHERE user_id = $1 AND id = ANY($2)",
        3 => "SELECT date, units, avg_price_per_unit, currency, isin, direction, withholding_tax, \
              withholding_tax_currency, eur_avg_price_per_unit, broker, hash, hash AS page_source_id \
              FROM trade WHERE user_id = $1 AND hash = ANY($2)",
        4 => "SELECT date, from_amount, to_amount, from_currency, to_currency, broker, id, \
              id AS page_source_id FROM fx_conversion WHERE user_id = $1 AND id = ANY($2)",
        5 => "SELECT t.id AS event_id, t.date, t.units, t.price_per_unit, t.eur_price_per_unit, \
              t.currency, t.direction, t.broker, a.id AS asset_id, a.name, a.unit_label, \
              t.id AS page_source_id FROM asset_trade t \
              JOIN asset a ON a.user_id = t.user_id AND a.id = t.asset_id \
              WHERE t.user_id = $1 AND t.id = ANY($2)",
        6 => "SELECT t.id AS event_id, t.date, t.kind, t.amount, t.amount_eur, t.currency, \
              a.id AS asset_id, a.name, a.unit_label, t.id AS page_source_id \
              FROM asset_transaction t JOIN asset a ON a.user_id = t.user_id AND a.id = t.asset_id \
              WHERE t.user_id = $1 AND t.id = ANY($2)",
        7 => "SELECT s.id AS event_id, s.date, s.kind, s.balance, s.balance_eur, a.currency, \
              a.id AS asset_id, a.name, a.unit_label, s.id AS page_source_id \
              FROM asset_balance_snapshot s \
              JOIN asset a ON a.user_id = s.user_id AND a.id = s.asset_id \
              WHERE s.user_id = $1 AND s.id = ANY($2)",
        8 => "SELECT v.id AS event_id, v.date, v.price_per_unit, v.eur_price_per_unit, \
              v.currency, a.id AS asset_id, a.name, a.unit_label, v.id AS page_source_id \
              FROM asset_valuation v JOIN asset a ON a.user_id = v.user_id AND a.id = v.asset_id \
              WHERE v.user_id = $1 AND v.id = ANY($2) AND v.source = 'Manual'",
        _ => anyhow::bail!("Unknown event page source: {source}"),
    };
    Ok(client.query(sql, &[&user_id.as_str(), &ids]).await?)
}
