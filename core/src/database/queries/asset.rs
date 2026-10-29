use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio_postgres::Row;

use crate::database::{
    db_client,
    models::asset::{
        Asset, AssetActivity, AssetBalanceSnapshot, AssetBalanceSnapshotKind, AssetClass,
        AssetDetailRecord, AssetListRecord, AssetTrade, AssetTradeDirection, AssetTransaction,
        AssetTransactionKind, AssetValuation, AssetValuationSource, CashAccountActivity,
        CreateAssetBalanceSnapshotRequest, CreateAssetRequest, CreateAssetTradeRequest,
        CreateAssetTransactionRequest, CreateAssetValuationRequest, CreateLinkedInterestRequest,
        InterestOrigin, LinkedAssetInterest, PrivateDebtActivity, TaxTreatment, TradeAssetActivity,
        UpdateAssetBalanceSnapshotRequest, UpdateAssetRequest, UpdateAssetTradeRequest,
        UpdateAssetTransactionRequest, UpdateAssetValuationRequest,
    },
};

const ASSET_COLUMNS: &str = "id, name, asset_class, currency, unit_label, \
    current_interest_rate_percent, interest_rate_updated_at, tax_treatment, archived_at, \
    created_at, updated_at";
const TRADE_COLUMNS: &str = "id, asset_id, date, units, price_per_unit, eur_price_per_unit, \
    direction, currency, broker, fees, fees_eur, note, created_at";
const VALUATION_COLUMNS: &str = "id, asset_id, date, price_per_unit, eur_price_per_unit, \
    currency, source, created_at";
const TRANSACTION_COLUMNS: &str = "id, asset_id, date, kind, amount, amount_eur, currency, \
    note, created_at";
const SNAPSHOT_COLUMNS: &str = "id, asset_id, date, kind, balance, balance_eur, note, created_at";
const INTEREST_COLUMNS: &str = "id, asset_id, date, amount, broker, principal, currency, \
    amount_eur, withholding_tax, withholding_tax_currency, tax_treatment, origin";

pub async fn create_asset(request: &CreateAssetRequest) -> anyhow::Result<Asset> {
    let client = db_client().await?;
    let asset_class = request.asset_class.as_str();
    let row = client
        .query_one(
            &format!(
                "INSERT INTO asset (name, asset_class, currency, unit_label, \
                 current_interest_rate_percent, interest_rate_updated_at) \
                 VALUES ($1, $2, $3, $4, $5, CASE WHEN $5::numeric IS NULL THEN NULL ELSE now() END) \
                 RETURNING {ASSET_COLUMNS}"
            ),
            &[
                &request.name,
                &asset_class,
                &request.currency,
                &request.unit_label,
                &request.current_interest_rate_percent,
            ],
        )
        .await?;
    row_to_asset(&row)
}

pub async fn update_asset(
    asset_id: &str,
    request: &UpdateAssetRequest,
) -> anyhow::Result<Option<Asset>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset SET name = $2, currency = $3, unit_label = $4, \
                 interest_rate_updated_at = CASE \
                    WHEN current_interest_rate_percent IS DISTINCT FROM $5 \
                    THEN now() ELSE interest_rate_updated_at END, \
                 current_interest_rate_percent = $5, updated_at = now() \
                 WHERE id = $1 RETURNING {ASSET_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.name,
                &request.currency,
                &request.unit_label,
                &request.current_interest_rate_percent,
            ],
        )
        .await?;
    row.as_ref().map(row_to_asset).transpose()
}

pub async fn list_assets(include_archived: bool) -> anyhow::Result<Vec<AssetListRecord>> {
    let client = db_client().await?;
    let statement = format!(
        "SELECT {ASSET_COLUMNS} FROM asset {} ORDER BY name, id",
        if include_archived {
            ""
        } else {
            "WHERE archived_at IS NULL"
        }
    );
    let rows = client.query(&statement, &[]).await?;
    rows.iter()
        .map(row_to_asset)
        .map(|asset| asset.map(AssetListRecord::from))
        .collect()
}

pub async fn get_asset(asset_id: &str) -> anyhow::Result<Option<Asset>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!("SELECT {ASSET_COLUMNS} FROM asset WHERE id = $1"),
            &[&asset_id],
        )
        .await?;
    row.as_ref().map(row_to_asset).transpose()
}

pub async fn get_asset_activity_revision(asset_id: &str) -> anyhow::Result<Option<i64>> {
    let client = db_client().await?;
    Ok(client
        .query_opt(
            "SELECT activity_revision FROM asset WHERE id = $1",
            &[&asset_id],
        )
        .await?
        .map(|row| row.get("activity_revision")))
}

pub async fn get_asset_detail(asset_id: &str) -> anyhow::Result<Option<AssetDetailRecord>> {
    let Some(asset) = get_asset(asset_id).await? else {
        return Ok(None);
    };

    let activity = match asset.asset_class {
        AssetClass::PhysicalGold => AssetActivity::PhysicalGold(TradeAssetActivity {
            trades: list_asset_trades(asset_id).await?,
            valuations: list_asset_valuations(asset_id).await?,
        }),
        AssetClass::RealEstate => AssetActivity::RealEstate(TradeAssetActivity {
            trades: list_asset_trades(asset_id).await?,
            valuations: list_asset_valuations(asset_id).await?,
        }),
        AssetClass::PrivateDebt => AssetActivity::PrivateDebt(PrivateDebtActivity {
            transactions: list_asset_transactions(asset_id).await?,
            interest: list_linked_interest(asset_id, None).await?,
        }),
        AssetClass::CashAccount => AssetActivity::CashAccount(CashAccountActivity {
            transactions: list_asset_transactions(asset_id).await?,
            balance_snapshots: list_asset_balance_snapshots(asset_id).await?,
            interest: list_linked_interest(asset_id, None).await?,
        }),
    };

    Ok(Some(AssetDetailRecord { asset, activity }))
}

pub async fn archive_asset(
    asset_id: &str,
    expected_activity_revision: i64,
) -> anyhow::Result<Option<Asset>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset SET archived_at = COALESCE(archived_at, now()), updated_at = now() \
                 WHERE id = $1 AND activity_revision = $2 RETURNING {ASSET_COLUMNS}"
            ),
            &[&asset_id, &expected_activity_revision],
        )
        .await?;
    row.as_ref().map(row_to_asset).transpose()
}

pub async fn restore_asset(asset_id: &str) -> anyhow::Result<Option<Asset>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset SET archived_at = NULL, updated_at = now() \
                 WHERE id = $1 RETURNING {ASSET_COLUMNS}"
            ),
            &[&asset_id],
        )
        .await?;
    row.as_ref().map(row_to_asset).transpose()
}

pub async fn list_asset_trades(asset_id: &str) -> anyhow::Result<Vec<AssetTrade>> {
    let client = db_client().await?;
    let rows = client
        .query(
            &format!(
                "SELECT {TRADE_COLUMNS} FROM asset_trade \
                  WHERE asset_id = $1 \
                  ORDER BY date, CASE WHEN direction = 'Buy' THEN 0 ELSE 1 END, created_at, id"
            ),
            &[&asset_id],
        )
        .await?;
    rows.iter().map(row_to_trade).collect()
}

pub async fn create_asset_trade(
    asset_id: &str,
    request: &CreateAssetTradeRequest,
    eur_price_per_unit: Decimal,
    fees_eur: Decimal,
) -> anyhow::Result<AssetTrade> {
    let mut client = db_client().await?;
    let transaction = client.transaction().await?;
    let direction = request.direction.as_str();
    let row = transaction
        .query_one(
            &format!(
                "INSERT INTO asset_trade (asset_id, date, units, price_per_unit, \
                 eur_price_per_unit, direction, currency, broker, fees, fees_eur, note) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
                 RETURNING {TRADE_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.date,
                &request.units,
                &request.price_per_unit,
                &eur_price_per_unit,
                &direction,
                &request.currency,
                &request.broker,
                &request.fees,
                &fees_eur,
                &request.note,
            ],
        )
        .await?;
    let trade = row_to_trade(&row)?;
    transaction
        .execute(
            "INSERT INTO asset_valuation \
             (id, asset_id, date, price_per_unit, eur_price_per_unit, currency, source) \
             VALUES ($1, $2, $3, $4, $5, $6, 'Trade')",
            &[
                &trade.id,
                &asset_id,
                &request.date,
                &request.price_per_unit,
                &eur_price_per_unit,
                &request.currency,
            ],
        )
        .await?;
    transaction.commit().await?;
    Ok(trade)
}

pub async fn update_asset_trade(
    asset_id: &str,
    trade_id: &str,
    request: &UpdateAssetTradeRequest,
    eur_price_per_unit: Decimal,
    fees_eur: Decimal,
) -> anyhow::Result<Option<AssetTrade>> {
    let mut client = db_client().await?;
    let transaction = client.transaction().await?;
    let direction = request.direction.as_str();
    let row = transaction
        .query_opt(
            &format!(
                "UPDATE asset_trade SET date = $3, units = $4, price_per_unit = $5, \
                 eur_price_per_unit = $6, direction = $7, currency = $8, broker = $9, \
                 fees = $10, fees_eur = $11, note = $12 WHERE asset_id = $1 AND id = $2 \
                 RETURNING {TRADE_COLUMNS}"
            ),
            &[
                &asset_id,
                &trade_id,
                &request.date,
                &request.units,
                &request.price_per_unit,
                &eur_price_per_unit,
                &direction,
                &request.currency,
                &request.broker,
                &request.fees,
                &fees_eur,
                &request.note,
            ],
        )
        .await?;

    let trade = row.as_ref().map(row_to_trade).transpose()?;
    if trade.is_some() {
        transaction
            .execute(
                "INSERT INTO asset_valuation \
                 (id, asset_id, date, price_per_unit, eur_price_per_unit, currency, source) \
                 VALUES ($1, $2, $3, $4, $5, $6, 'Trade') \
                 ON CONFLICT (id) DO UPDATE SET date = EXCLUDED.date, \
                 price_per_unit = EXCLUDED.price_per_unit, \
                 eur_price_per_unit = EXCLUDED.eur_price_per_unit, currency = EXCLUDED.currency",
                &[
                    &trade_id,
                    &asset_id,
                    &request.date,
                    &request.price_per_unit,
                    &eur_price_per_unit,
                    &request.currency,
                ],
            )
            .await?;
    }
    transaction.commit().await?;
    Ok(trade)
}

pub async fn delete_asset_trade(asset_id: &str, trade_id: &str) -> anyhow::Result<bool> {
    let mut client = db_client().await?;
    let transaction = client.transaction().await?;
    transaction
        .execute(
            "DELETE FROM asset_valuation WHERE asset_id = $1 AND id = $2 AND source = 'Trade'",
            &[&asset_id, &trade_id],
        )
        .await?;
    let deleted = transaction
        .execute(
            "DELETE FROM asset_trade WHERE asset_id = $1 AND id = $2",
            &[&asset_id, &trade_id],
        )
        .await?
        == 1;
    transaction.commit().await?;
    Ok(deleted)
}

pub async fn list_asset_valuations(asset_id: &str) -> anyhow::Result<Vec<AssetValuation>> {
    let client = db_client().await?;
    let rows = client
        .query(
            &format!(
                "SELECT {VALUATION_COLUMNS} FROM asset_valuation \
                 WHERE asset_id = $1 ORDER BY date, created_at, id"
            ),
            &[&asset_id],
        )
        .await?;
    rows.iter().map(row_to_valuation).collect()
}

pub async fn create_asset_valuation(
    asset_id: &str,
    request: &CreateAssetValuationRequest,
    eur_price_per_unit: Decimal,
) -> anyhow::Result<AssetValuation> {
    let client = db_client().await?;
    let row = client
        .query_one(
            &format!(
                "INSERT INTO asset_valuation (asset_id, date, price_per_unit, \
                 eur_price_per_unit, currency, source) VALUES ($1, $2, $3, $4, $5, 'Manual') \
                 RETURNING {VALUATION_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.date,
                &request.price_per_unit,
                &eur_price_per_unit,
                &request.currency,
            ],
        )
        .await?;
    row_to_valuation(&row)
}

pub async fn update_asset_valuation(
    asset_id: &str,
    valuation_id: &str,
    request: &UpdateAssetValuationRequest,
    eur_price_per_unit: Decimal,
) -> anyhow::Result<Option<AssetValuation>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset_valuation SET date = $3, price_per_unit = $4, \
                 eur_price_per_unit = $5, currency = $6 \
                 WHERE asset_id = $1 AND id = $2 AND source = 'Manual' \
                 RETURNING {VALUATION_COLUMNS}"
            ),
            &[
                &asset_id,
                &valuation_id,
                &request.date,
                &request.price_per_unit,
                &eur_price_per_unit,
                &request.currency,
            ],
        )
        .await?;
    row.as_ref().map(row_to_valuation).transpose()
}

pub async fn delete_asset_valuation(asset_id: &str, valuation_id: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    Ok(client
        .execute(
            "DELETE FROM asset_valuation \
             WHERE asset_id = $1 AND id = $2 AND source = 'Manual'",
            &[&asset_id, &valuation_id],
        )
        .await?
        == 1)
}

pub async fn list_asset_transactions(asset_id: &str) -> anyhow::Result<Vec<AssetTransaction>> {
    let client = db_client().await?;
    let rows = client
        .query(
            &format!(
                "SELECT {TRANSACTION_COLUMNS} FROM asset_transaction \
                 WHERE asset_id = $1 ORDER BY date, created_at, id"
            ),
            &[&asset_id],
        )
        .await?;
    rows.iter().map(row_to_transaction).collect()
}

pub async fn create_asset_transaction(
    asset_id: &str,
    request: &CreateAssetTransactionRequest,
    amount_eur: Decimal,
) -> anyhow::Result<AssetTransaction> {
    let client = db_client().await?;
    let kind = request.kind.as_str();
    let row = client
        .query_one(
            &format!(
                "INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, \
                 currency, note) VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 RETURNING {TRANSACTION_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.date,
                &kind,
                &request.amount,
                &amount_eur,
                &request.currency,
                &request.note,
            ],
        )
        .await?;
    row_to_transaction(&row)
}

pub async fn update_asset_transaction(
    asset_id: &str,
    transaction_id: &str,
    request: &UpdateAssetTransactionRequest,
    amount_eur: Decimal,
) -> anyhow::Result<Option<AssetTransaction>> {
    let client = db_client().await?;
    let kind = request.kind.as_str();
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset_transaction SET date = $3, kind = $4, amount = $5, \
                 amount_eur = $6, currency = $7, note = $8 \
                 WHERE asset_id = $1 AND id = $2 RETURNING {TRANSACTION_COLUMNS}"
            ),
            &[
                &asset_id,
                &transaction_id,
                &request.date,
                &kind,
                &request.amount,
                &amount_eur,
                &request.currency,
                &request.note,
            ],
        )
        .await?;
    row.as_ref().map(row_to_transaction).transpose()
}

pub async fn delete_asset_transaction(
    asset_id: &str,
    transaction_id: &str,
) -> anyhow::Result<bool> {
    let client = db_client().await?;
    Ok(client
        .execute(
            "DELETE FROM asset_transaction WHERE asset_id = $1 AND id = $2",
            &[&asset_id, &transaction_id],
        )
        .await?
        == 1)
}

pub async fn list_asset_balance_snapshots(
    asset_id: &str,
) -> anyhow::Result<Vec<AssetBalanceSnapshot>> {
    let client = db_client().await?;
    let rows = client
        .query(
            &format!(
                "SELECT {SNAPSHOT_COLUMNS} FROM asset_balance_snapshot \
                 WHERE asset_id = $1 ORDER BY date, created_at, id"
            ),
            &[&asset_id],
        )
        .await?;
    rows.iter().map(row_to_snapshot).collect()
}

pub async fn create_asset_balance_snapshot(
    asset_id: &str,
    request: &CreateAssetBalanceSnapshotRequest,
    balance_eur: Decimal,
) -> anyhow::Result<AssetBalanceSnapshot> {
    let client = db_client().await?;
    let kind = request.kind.as_str();
    let row = client
        .query_one(
            &format!(
                "INSERT INTO asset_balance_snapshot (asset_id, date, kind, balance, \
                 balance_eur, note) VALUES ($1, $2, $3, $4, $5, $6) \
                 RETURNING {SNAPSHOT_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.date,
                &kind,
                &request.balance,
                &balance_eur,
                &request.note,
            ],
        )
        .await?;
    row_to_snapshot(&row)
}

pub async fn update_asset_balance_snapshot(
    asset_id: &str,
    snapshot_id: &str,
    request: &UpdateAssetBalanceSnapshotRequest,
    balance_eur: Decimal,
) -> anyhow::Result<Option<AssetBalanceSnapshot>> {
    let client = db_client().await?;
    let kind = request.kind.as_str();
    let row = client
        .query_opt(
            &format!(
                "UPDATE asset_balance_snapshot SET date = $3, kind = $4, balance = $5, \
                 balance_eur = $6, note = $7 WHERE asset_id = $1 AND id = $2 \
                 RETURNING {SNAPSHOT_COLUMNS}"
            ),
            &[
                &asset_id,
                &snapshot_id,
                &request.date,
                &kind,
                &request.balance,
                &balance_eur,
                &request.note,
            ],
        )
        .await?;
    row.as_ref().map(row_to_snapshot).transpose()
}

pub async fn delete_asset_balance_snapshot(
    asset_id: &str,
    snapshot_id: &str,
) -> anyhow::Result<bool> {
    let client = db_client().await?;
    Ok(client
        .execute(
            "DELETE FROM asset_balance_snapshot WHERE asset_id = $1 AND id = $2",
            &[&asset_id, &snapshot_id],
        )
        .await?
        == 1)
}

pub async fn get_interest_by_id(interest_id: &str) -> anyhow::Result<Option<LinkedAssetInterest>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!("SELECT {INTEREST_COLUMNS} FROM interest WHERE id = $1"),
            &[&interest_id],
        )
        .await?;
    row.as_ref().map(row_to_interest).transpose()
}

pub async fn list_linked_interest(
    asset_id: &str,
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<LinkedAssetInterest>> {
    let client = db_client().await?;
    let (statement, params): (String, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
        if let Some(ref as_of) = as_of {
            (
                format!(
                    "SELECT {INTEREST_COLUMNS} FROM interest \
                     WHERE asset_id = $1 AND date <= $2 ORDER BY date, id"
                ),
                vec![&asset_id, as_of],
            )
        } else {
            (
                format!(
                    "SELECT {INTEREST_COLUMNS} FROM interest \
                     WHERE asset_id = $1 ORDER BY date, id"
                ),
                vec![&asset_id],
            )
        };
    let rows = client.query(&statement, &params).await?;
    rows.iter().map(row_to_interest).collect()
}

pub async fn link_interest(asset_id: &str, interest_id: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    Ok(client
        .execute(
            "UPDATE interest SET asset_id = $1 \
             WHERE id = $2 AND (asset_id IS NULL OR asset_id = $1)",
            &[&asset_id, &interest_id],
        )
        .await?
        == 1)
}

pub async fn unlink_interest(asset_id: &str, interest_id: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    let deleted = client
        .execute(
            "DELETE FROM interest WHERE id = $1 AND asset_id = $2 AND origin = 'Manual'",
            &[&interest_id, &asset_id],
        )
        .await?;
    if deleted == 1 {
        return Ok(true);
    }
    Ok(client
        .execute(
            "UPDATE interest SET asset_id = NULL \
             WHERE id = $1 AND asset_id = $2 AND origin = 'Imported'",
            &[&interest_id, &asset_id],
        )
        .await?
        == 1)
}

pub async fn create_linked_interest(
    asset_id: &str,
    request: &CreateLinkedInterestRequest,
    amount_eur: Decimal,
) -> anyhow::Result<LinkedAssetInterest> {
    let client = db_client().await?;
    let row = client
        .query_one(
            &format!(
                "INSERT INTO interest (id, asset_id, date, amount, broker, principal, currency, \
                  amount_eur, withholding_tax, withholding_tax_currency, tax_treatment, origin) \
                  VALUES (gen_random_uuid()::text, $1, $2, $3, $4, $5, $6, $7, $8, \
                   $9, 'Excluded', 'Manual') \
                 RETURNING {INTEREST_COLUMNS}"
            ),
            &[
                &asset_id,
                &request.date,
                &request.amount,
                &request.broker,
                &request.principal,
                &request.currency,
                &amount_eur,
                &request.withholding_tax,
                &request.withholding_tax_currency,
            ],
        )
        .await?;
    row_to_interest(&row)
}

pub async fn update_linked_interest(
    asset_id: &str,
    interest_id: &str,
    request: &CreateLinkedInterestRequest,
    amount_eur: Decimal,
) -> anyhow::Result<Option<LinkedAssetInterest>> {
    let client = db_client().await?;
    let row = client
        .query_opt(
            &format!(
                "UPDATE interest SET date = $3, amount = $4, broker = $5, principal = $6, \
                 currency = $7, amount_eur = $8, withholding_tax = $9, \
                 withholding_tax_currency = $10 \
                 WHERE asset_id = $1 AND id = $2 AND origin = 'Manual' \
                 RETURNING {INTEREST_COLUMNS}"
            ),
            &[
                &asset_id,
                &interest_id,
                &request.date,
                &request.amount,
                &request.broker,
                &request.principal,
                &request.currency,
                &amount_eur,
                &request.withholding_tax,
                &request.withholding_tax_currency,
            ],
        )
        .await?;
    row.as_ref().map(row_to_interest).transpose()
}

fn row_to_asset(row: &Row) -> anyhow::Result<Asset> {
    Ok(Asset {
        id: row.get("id"),
        name: row.get("name"),
        asset_class: AssetClass::from_str(row.get::<_, &str>("asset_class"))?,
        currency: row.get("currency"),
        unit_label: row.get("unit_label"),
        current_interest_rate_percent: row.get("current_interest_rate_percent"),
        interest_rate_updated_at: row.get("interest_rate_updated_at"),
        tax_treatment: TaxTreatment::from_str(row.get::<_, &str>("tax_treatment"))?,
        archived_at: row.get("archived_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_trade(row: &Row) -> anyhow::Result<AssetTrade> {
    Ok(AssetTrade {
        id: row.get("id"),
        asset_id: row.get("asset_id"),
        date: row.get("date"),
        units: row.get("units"),
        price_per_unit: row.get("price_per_unit"),
        eur_price_per_unit: row.get("eur_price_per_unit"),
        direction: AssetTradeDirection::from_str(row.get::<_, &str>("direction"))?,
        currency: row.get("currency"),
        broker: row.get("broker"),
        fees: row.get("fees"),
        fees_eur: row.get("fees_eur"),
        note: row.get("note"),
        created_at: row.get("created_at"),
    })
}

fn row_to_valuation(row: &Row) -> anyhow::Result<AssetValuation> {
    Ok(AssetValuation {
        id: row.get("id"),
        asset_id: row.get("asset_id"),
        date: row.get("date"),
        price_per_unit: row.get("price_per_unit"),
        eur_price_per_unit: row.get("eur_price_per_unit"),
        currency: row.get("currency"),
        source: AssetValuationSource::from_str(row.get::<_, &str>("source"))?,
        created_at: row.get("created_at"),
    })
}

fn row_to_transaction(row: &Row) -> anyhow::Result<AssetTransaction> {
    Ok(AssetTransaction {
        id: row.get("id"),
        asset_id: row.get("asset_id"),
        date: row.get("date"),
        kind: AssetTransactionKind::from_str(row.get::<_, &str>("kind"))?,
        amount: row.get("amount"),
        amount_eur: row.get("amount_eur"),
        currency: row.get("currency"),
        note: row.get("note"),
        created_at: row.get("created_at"),
    })
}

fn row_to_snapshot(row: &Row) -> anyhow::Result<AssetBalanceSnapshot> {
    Ok(AssetBalanceSnapshot {
        id: row.get("id"),
        asset_id: row.get("asset_id"),
        date: row.get("date"),
        kind: AssetBalanceSnapshotKind::from_str(row.get::<_, &str>("kind"))?,
        balance: row.get("balance"),
        balance_eur: row.get("balance_eur"),
        note: row.get("note"),
        created_at: row.get("created_at"),
    })
}

fn row_to_interest(row: &Row) -> anyhow::Result<LinkedAssetInterest> {
    Ok(LinkedAssetInterest {
        id: row.get("id"),
        asset_id: row.get("asset_id"),
        date: row.get("date"),
        amount: row.get("amount"),
        broker: row.get("broker"),
        principal: row.get("principal"),
        currency: row.get("currency"),
        amount_eur: row.get("amount_eur"),
        withholding_tax: row.get("withholding_tax"),
        withholding_tax_currency: row.get("withholding_tax_currency"),
        tax_treatment: TaxTreatment::from_str(row.get::<_, &str>("tax_treatment"))?,
        origin: InterestOrigin::from_str(row.get::<_, &str>("origin"))?,
    })
}
