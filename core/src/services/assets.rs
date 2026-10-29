use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::{
    database::{
        models::asset::{
            Asset, AssetActivity, AssetBalanceSnapshot, AssetBalanceSnapshotKind, AssetClass,
            AssetDetailRecord, AssetListRecord, AssetTrade, AssetTradeDirection, AssetTransaction,
            AssetTransactionKind, AssetValuation, CashAccountSummary,
            CreateAssetBalanceSnapshotRequest, CreateAssetRequest, CreateAssetTradeRequest,
            CreateAssetTransactionRequest, CreateAssetValuationRequest,
            CreateLinkedInterestRequest, CustomAssetHolding, CustomAssetPortfolioSummary,
            LinkedAssetInterest, UpdateAssetBalanceSnapshotRequest, UpdateAssetRequest,
            UpdateAssetTradeRequest, UpdateAssetTransactionRequest, UpdateAssetValuationRequest,
            WeightedCashInterestSummary,
        },
        queries::asset as asset_queries,
    },
    services::market_data::fx_rates::convert_amount,
};

pub async fn create_asset(request: &CreateAssetRequest) -> anyhow::Result<Asset> {
    request.validate()?;
    asset_queries::create_asset(request).await
}

pub async fn update_asset(asset_id: &str, request: &UpdateAssetRequest) -> anyhow::Result<Asset> {
    let asset = require_asset(asset_id).await?;
    request.validate_for(asset.asset_class)?;

    if request.currency != asset.currency || request.unit_label != asset.unit_label {
        let detail = asset_queries::get_asset_detail(asset_id)
            .await?
            .context("asset disappeared while being updated")?;
        if detail_has_activity(&detail) {
            if request.currency != asset.currency {
                bail!("asset currency cannot change after activity has been recorded");
            }
            if request.unit_label != asset.unit_label {
                bail!("asset unit label cannot change after activity has been recorded");
            }
        }
    }

    asset_queries::update_asset(asset_id, request)
        .await?
        .context("asset disappeared while being updated")
}

pub async fn list_assets(include_archived: bool) -> anyhow::Result<Vec<AssetListRecord>> {
    asset_queries::list_assets(include_archived).await
}

pub async fn get_asset_detail(asset_id: &str) -> anyhow::Result<Option<AssetDetailRecord>> {
    asset_queries::get_asset_detail(asset_id).await
}

pub async fn archive_asset(asset_id: &str) -> anyhow::Result<Asset> {
    let activity_revision = asset_queries::get_asset_activity_revision(asset_id)
        .await?
        .with_context(|| format!("asset not found: {asset_id}"))?;
    let detail = asset_queries::get_asset_detail(asset_id)
        .await?
        .with_context(|| format!("asset not found: {asset_id}"))?;
    let now = Utc::now();
    if detail_has_future_activity(&detail, now) {
        bail!("asset with future-dated activity cannot be archived");
    }
    let holding = calculate_holding(detail, now).await?;
    if holding.units_or_balance != Decimal::ZERO {
        bail!("asset must have a zero balance before it can be archived");
    }
    if let Some(asset) = asset_queries::archive_asset(asset_id, activity_revision).await? {
        return Ok(asset);
    }
    if asset_queries::get_asset(asset_id).await?.is_none() {
        bail!("asset not found: {asset_id}");
    }
    bail!("asset activity changed while it was being archived; try again")
}

pub async fn restore_asset(asset_id: &str) -> anyhow::Result<Asset> {
    asset_queries::restore_asset(asset_id)
        .await?
        .with_context(|| format!("asset not found: {asset_id}"))
}

pub async fn create_asset_trade(
    asset_id: &str,
    request: &CreateAssetTradeRequest,
) -> anyhow::Result<AssetTrade> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    ensure_sell_inventory(
        asset_id,
        request.date,
        request.units,
        request.direction,
        None,
    )
    .await?;
    let eur_price_per_unit = resolve_eur(
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    let fees_eur = resolve_trade_fees_eur(
        request.fees,
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::create_asset_trade(asset_id, request, eur_price_per_unit, fees_eur).await
}

pub async fn update_asset_trade(
    asset_id: &str,
    trade_id: &str,
    request: &UpdateAssetTradeRequest,
) -> anyhow::Result<Option<AssetTrade>> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    ensure_sell_inventory(
        asset_id,
        request.date,
        request.units,
        request.direction,
        Some(trade_id),
    )
    .await?;
    let eur_price_per_unit = resolve_eur(
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    let fees_eur = resolve_trade_fees_eur(
        request.fees,
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::update_asset_trade(asset_id, trade_id, request, eur_price_per_unit, fees_eur)
        .await
}

pub async fn delete_asset_trade(asset_id: &str, trade_id: &str) -> anyhow::Result<bool> {
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    asset_queries::delete_asset_trade(asset_id, trade_id).await
}

pub async fn create_asset_valuation(
    asset_id: &str,
    request: &CreateAssetValuationRequest,
) -> anyhow::Result<AssetValuation> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    let eur_price_per_unit = resolve_eur(
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::create_asset_valuation(asset_id, request, eur_price_per_unit).await
}

pub async fn update_asset_valuation(
    asset_id: &str,
    valuation_id: &str,
    request: &UpdateAssetValuationRequest,
) -> anyhow::Result<Option<AssetValuation>> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    let eur_price_per_unit = resolve_eur(
        request.price_per_unit,
        request.eur_price_per_unit,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::update_asset_valuation(asset_id, valuation_id, request, eur_price_per_unit).await
}

pub async fn delete_asset_valuation(asset_id: &str, valuation_id: &str) -> anyhow::Result<bool> {
    let asset = require_active_asset(asset_id).await?;
    ensure_trade_asset(&asset)?;
    asset_queries::delete_asset_valuation(asset_id, valuation_id).await
}

pub async fn create_asset_transaction(
    asset_id: &str,
    request: &CreateAssetTransactionRequest,
) -> anyhow::Result<AssetTransaction> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_transaction_kind(&asset, request.kind)?;
    ensure_currency(&asset, &request.currency)?;
    ensure_available_balance(&asset, request.date, request.kind, request.amount, None).await?;
    let amount_eur = resolve_eur(
        request.amount,
        request.amount_eur,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::create_asset_transaction(asset_id, request, amount_eur).await
}

pub async fn update_asset_transaction(
    asset_id: &str,
    transaction_id: &str,
    request: &UpdateAssetTransactionRequest,
) -> anyhow::Result<Option<AssetTransaction>> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_transaction_kind(&asset, request.kind)?;
    ensure_currency(&asset, &request.currency)?;
    ensure_available_balance(
        &asset,
        request.date,
        request.kind,
        request.amount,
        Some(transaction_id),
    )
    .await?;
    let amount_eur = resolve_eur(
        request.amount,
        request.amount_eur,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::update_asset_transaction(asset_id, transaction_id, request, amount_eur).await
}

pub async fn delete_asset_transaction(
    asset_id: &str,
    transaction_id: &str,
) -> anyhow::Result<bool> {
    let asset = require_active_asset(asset_id).await?;
    if !asset.asset_class.supports_transactions() {
        bail!(
            "{} does not support principal transactions",
            asset.asset_class
        );
    }
    asset_queries::delete_asset_transaction(asset_id, transaction_id).await
}

pub async fn create_asset_balance_snapshot(
    asset_id: &str,
    request: &CreateAssetBalanceSnapshotRequest,
) -> anyhow::Result<AssetBalanceSnapshot> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_cash_account(&asset)?;
    let balance_eur = resolve_eur(
        request.balance,
        request.balance_eur,
        request.date,
        &asset.currency,
    )
    .await?;
    asset_queries::create_asset_balance_snapshot(asset_id, request, balance_eur).await
}

pub async fn update_asset_balance_snapshot(
    asset_id: &str,
    snapshot_id: &str,
    request: &UpdateAssetBalanceSnapshotRequest,
) -> anyhow::Result<Option<AssetBalanceSnapshot>> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_cash_account(&asset)?;
    let balance_eur = resolve_eur(
        request.balance,
        request.balance_eur,
        request.date,
        &asset.currency,
    )
    .await?;
    asset_queries::update_asset_balance_snapshot(asset_id, snapshot_id, request, balance_eur).await
}

pub async fn delete_asset_balance_snapshot(
    asset_id: &str,
    snapshot_id: &str,
) -> anyhow::Result<bool> {
    let asset = require_active_asset(asset_id).await?;
    ensure_cash_account(&asset)?;
    asset_queries::delete_asset_balance_snapshot(asset_id, snapshot_id).await
}

pub async fn link_interest(asset_id: &str, interest_id: &str) -> anyhow::Result<bool> {
    let asset = require_active_asset(asset_id).await?;
    ensure_interest_asset(&asset)?;
    let interest = asset_queries::get_interest_by_id(interest_id)
        .await?
        .with_context(|| format!("interest not found: {interest_id}"))?;
    ensure_currency(&asset, &interest.currency)?;
    ensure_interest_principal(&asset, interest.principal.as_deref())?;
    asset_queries::link_interest(asset_id, interest_id).await
}

pub async fn unlink_interest(asset_id: &str, interest_id: &str) -> anyhow::Result<bool> {
    let asset = require_asset(asset_id).await?;
    ensure_interest_asset(&asset)?;
    asset_queries::unlink_interest(asset_id, interest_id).await
}

pub async fn create_linked_interest(
    asset_id: &str,
    request: &CreateLinkedInterestRequest,
) -> anyhow::Result<LinkedAssetInterest> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_interest_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    let mut request = request.clone();
    if request.principal.is_none() {
        request.principal = Some(
            if asset.asset_class == AssetClass::CashAccount {
                "Cash"
            } else {
                "PrivateDebt"
            }
            .to_string(),
        );
    }
    ensure_interest_principal(&asset, request.principal.as_deref())?;
    let amount_eur = resolve_eur(
        request.amount,
        request.amount_eur,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::create_linked_interest(asset_id, &request, amount_eur).await
}

pub async fn update_linked_interest(
    asset_id: &str,
    interest_id: &str,
    request: &CreateLinkedInterestRequest,
) -> anyhow::Result<Option<LinkedAssetInterest>> {
    request.validate()?;
    let asset = require_active_asset(asset_id).await?;
    ensure_interest_asset(&asset)?;
    ensure_currency(&asset, &request.currency)?;
    let mut request = request.clone();
    if request.principal.is_none() {
        request.principal = Some(
            if asset.asset_class == AssetClass::CashAccount {
                "Cash"
            } else {
                "PrivateDebt"
            }
            .to_string(),
        );
    }
    ensure_interest_principal(&asset, request.principal.as_deref())?;
    let amount_eur = resolve_eur(
        request.amount,
        request.amount_eur,
        request.date,
        &request.currency,
    )
    .await?;
    asset_queries::update_linked_interest(asset_id, interest_id, &request, amount_eur).await
}

pub async fn calculate_custom_holdings() -> anyhow::Result<Vec<CustomAssetHolding>> {
    calculate_custom_holdings_as_of(Utc::now()).await
}

pub async fn calculate_custom_holdings_as_of(
    as_of: DateTime<Utc>,
) -> anyhow::Result<Vec<CustomAssetHolding>> {
    let assets = asset_queries::list_assets(true).await?;
    let mut holdings = Vec::with_capacity(assets.len());

    for listed_asset in assets {
        let detail = asset_queries::get_asset_detail(&listed_asset.id)
            .await?
            .with_context(|| {
                format!("asset disappeared during calculation: {}", listed_asset.id)
            })?;
        holdings.push(calculate_holding(detail, as_of).await?);
    }

    Ok(holdings)
}

/// Totals include every known value. Callers should treat the result as incomplete whenever
/// `incomplete_asset_count` is non-zero rather than silently valuing missing holdings at zero.
pub fn summarize_custom_holdings(holdings: &[CustomAssetHolding]) -> CustomAssetPortfolioSummary {
    let mut current_value_eur = Decimal::ZERO;
    let mut invested_eur = Decimal::ZERO;
    let mut total_return_eur = Decimal::ZERO;
    let mut known_invested_eur = Decimal::ZERO;
    let mut incomplete_asset_count = 0_i64;

    for holding in holdings {
        invested_eur += holding.invested_eur;
        match (holding.current_value_eur, holding.total_return_eur) {
            (Some(current_value), Some(total_return)) => {
                current_value_eur += current_value;
                total_return_eur += total_return;
                known_invested_eur += holding.invested_eur;
            }
            _ => incomplete_asset_count += 1,
        }
    }

    CustomAssetPortfolioSummary {
        current_value_eur,
        invested_eur,
        known_invested_eur,
        total_return_eur,
        incomplete_asset_count,
    }
}

pub async fn calculate_weighted_cash_interest() -> anyhow::Result<WeightedCashInterestSummary> {
    let as_of = Utc::now();
    let assets = asset_queries::list_assets(false).await?;
    let mut accounts = Vec::new();

    for listed_asset in assets
        .into_iter()
        .filter(|asset| asset.asset_class == AssetClass::CashAccount)
    {
        let detail = asset_queries::get_asset_detail(&listed_asset.id)
            .await?
            .with_context(|| {
                format!("asset disappeared during calculation: {}", listed_asset.id)
            })?;
        let AssetActivity::CashAccount(activity) = detail.activity else {
            bail!("cash account returned incompatible activity");
        };
        let folded = fold_cash_events(
            &activity.transactions,
            &activity.balance_snapshots,
            &activity.interest,
            as_of,
        );
        let current_balance_eur = try_convert_current(
            folded.balance,
            as_of,
            &detail.asset.currency,
            &detail.asset.id,
        )
        .await;
        accounts.push(CashAccountSummary {
            asset_id: detail.asset.id,
            name: detail.asset.name,
            currency: detail.asset.currency,
            current_balance: folded.balance,
            current_balance_eur,
            current_interest_rate_percent: detail
                .asset
                .current_interest_rate_percent
                .context("cash account has no current interest rate")?,
            interest_rate_updated_at: detail.asset.interest_rate_updated_at,
        });
    }

    Ok(weighted_cash_interest_summary(accounts))
}

pub fn weighted_cash_interest_summary(
    accounts: Vec<CashAccountSummary>,
) -> WeightedCashInterestSummary {
    let mut eligible_balance_eur = Decimal::ZERO;
    let mut weighted_rates = Decimal::ZERO;
    let mut unconverted_count = 0_i64;

    for account in &accounts {
        match account.current_balance_eur {
            Some(balance_eur) if balance_eur > Decimal::ZERO => {
                eligible_balance_eur += balance_eur;
                weighted_rates += balance_eur * account.current_interest_rate_percent;
            }
            None if account.current_balance > Decimal::ZERO => unconverted_count += 1,
            _ => {}
        }
    }

    WeightedCashInterestSummary {
        average_interest_rate_percent: if eligible_balance_eur > Decimal::ZERO {
            Some(weighted_rates / eligible_balance_eur)
        } else {
            None
        },
        eligible_balance_eur,
        unconverted_count,
        accounts,
    }
}

async fn calculate_holding(
    detail: AssetDetailRecord,
    as_of: DateTime<Utc>,
) -> anyhow::Result<CustomAssetHolding> {
    let AssetDetailRecord { asset, activity } = detail;
    match activity {
        AssetActivity::PhysicalGold(activity) | AssetActivity::RealEstate(activity) => {
            calculate_trade_holding(asset, activity.trades, activity.valuations, as_of).await
        }
        AssetActivity::PrivateDebt(activity) => {
            Ok(
                calculate_debt_holding(asset, &activity.transactions, &activity.interest, as_of)
                    .await,
            )
        }
        AssetActivity::CashAccount(activity) => Ok(calculate_cash_holding(
            asset,
            &activity.transactions,
            &activity.balance_snapshots,
            &activity.interest,
            as_of,
        )
        .await),
    }
}

async fn calculate_trade_holding(
    asset: Asset,
    trades: Vec<AssetTrade>,
    valuations: Vec<AssetValuation>,
    as_of: DateTime<Utc>,
) -> anyhow::Result<CustomAssetHolding> {
    let mut units = Decimal::ZERO;
    let mut cost_basis_eur = Decimal::ZERO;
    let mut net_contributions_eur = Decimal::ZERO;
    let mut invested_eur = Decimal::ZERO;
    let mut realized_return_eur = Decimal::ZERO;

    for trade in trades.iter().filter(|trade| trade.date <= as_of) {
        let fees_eur = trade.fees_eur;
        match trade.direction {
            AssetTradeDirection::Buy => {
                let purchase_cost = trade.eur_price_per_unit * trade.units + fees_eur;
                units += trade.units;
                cost_basis_eur += purchase_cost;
                net_contributions_eur += purchase_cost;
                invested_eur += purchase_cost;
            }
            AssetTradeDirection::Sell => {
                if trade.units > units {
                    bail!(
                        "asset {} sells {} units on {} but only {} are held",
                        asset.id,
                        trade.units,
                        trade.date,
                        units
                    );
                }
                let average_cost = if units > Decimal::ZERO {
                    cost_basis_eur / units
                } else {
                    Decimal::ZERO
                };
                let removed_cost = average_cost * trade.units;
                let proceeds = trade.eur_price_per_unit * trade.units - fees_eur;
                units -= trade.units;
                cost_basis_eur -= removed_cost;
                net_contributions_eur -= proceeds;
                realized_return_eur += proceeds - removed_cost;
            }
        }
    }

    let valuation = valuations
        .iter()
        .filter(|valuation| valuation.date <= as_of)
        .max_by_key(|valuation| (valuation.date, valuation.created_at, valuation.id.as_str()));
    let (current_value, current_value_eur, valuation_date) = if units == Decimal::ZERO {
        (Some(Decimal::ZERO), Some(Decimal::ZERO), None)
    } else if let Some(valuation) = valuation {
        (
            Some(valuation.price_per_unit * units),
            Some(valuation.eur_price_per_unit * units),
            Some(valuation.date),
        )
    } else {
        (None, None, None)
    };
    let unrealized_return_eur = current_value_eur.map(|value| value - cost_basis_eur);
    let total_return_eur = unrealized_return_eur.map(|unrealized| realized_return_eur + unrealized);

    Ok(CustomAssetHolding {
        asset_id: asset.id,
        name: asset.name,
        asset_class: asset.asset_class,
        currency: asset.currency,
        unit_label: asset.unit_label,
        units_or_balance: units,
        current_value,
        current_value_eur,
        net_contributions_eur,
        invested_eur,
        realized_return_eur,
        unrealized_return_eur,
        total_return_eur,
        income_eur: Decimal::ZERO,
        valuation_date,
    })
}

async fn calculate_cash_holding(
    asset: Asset,
    transactions: &[AssetTransaction],
    snapshots: &[AssetBalanceSnapshot],
    interest: &[LinkedAssetInterest],
    as_of: DateTime<Utc>,
) -> CustomAssetHolding {
    let folded = fold_cash_events(transactions, snapshots, interest, as_of);
    let current_value_eur =
        try_convert_current(folded.balance, as_of, &asset.currency, &asset.id).await;
    let total_return_eur = current_value_eur.map(|value| value - folded.net_contributions_eur);
    let realized_return_eur = folded.income_eur + folded.realized_fx_eur;
    let unrealized_return_eur = total_return_eur.map(|total| total - realized_return_eur);

    CustomAssetHolding {
        asset_id: asset.id,
        name: asset.name,
        asset_class: asset.asset_class,
        currency: asset.currency,
        unit_label: asset.unit_label,
        units_or_balance: folded.balance,
        current_value: Some(folded.balance),
        current_value_eur,
        net_contributions_eur: folded.net_contributions_eur,
        invested_eur: folded.invested_eur,
        realized_return_eur,
        unrealized_return_eur,
        total_return_eur,
        income_eur: folded.income_eur,
        valuation_date: folded.last_snapshot_date,
    }
}

async fn calculate_debt_holding(
    asset: Asset,
    transactions: &[AssetTransaction],
    interest: &[LinkedAssetInterest],
    as_of: DateTime<Utc>,
) -> CustomAssetHolding {
    let mut principal = Decimal::ZERO;
    let mut principal_cost_basis_eur = Decimal::ZERO;
    let mut net_contributions_eur = Decimal::ZERO;
    let mut invested_eur = Decimal::ZERO;
    let mut realized_fx_eur = Decimal::ZERO;
    for transaction in transactions
        .iter()
        .filter(|transaction| transaction.date <= as_of)
    {
        match transaction.kind {
            AssetTransactionKind::PrincipalAdvance => {
                principal += transaction.amount;
                principal_cost_basis_eur += transaction.amount_eur;
                net_contributions_eur += transaction.amount_eur;
                invested_eur += transaction.amount_eur;
            }
            AssetTransactionKind::PrincipalRepayment => {
                let removed_cost_basis = if principal > Decimal::ZERO {
                    principal_cost_basis_eur * transaction.amount / principal
                } else {
                    Decimal::ZERO
                };
                principal -= transaction.amount;
                principal_cost_basis_eur -= removed_cost_basis;
                net_contributions_eur -= transaction.amount_eur;
                realized_fx_eur += transaction.amount_eur - removed_cost_basis;
            }
            AssetTransactionKind::Deposit | AssetTransactionKind::Withdrawal => {}
        }
    }
    let income_eur = interest
        .iter()
        .filter(|interest| interest.date <= as_of)
        .map(|interest| interest.amount_eur)
        .sum();
    let current_value_eur = try_convert_current(principal, as_of, &asset.currency, &asset.id).await;
    let total_return_eur =
        current_value_eur.map(|value| value + income_eur - net_contributions_eur);
    let realized_return_eur = income_eur + realized_fx_eur;
    let unrealized_return_eur = current_value_eur.map(|value| value - principal_cost_basis_eur);

    CustomAssetHolding {
        asset_id: asset.id,
        name: asset.name,
        asset_class: asset.asset_class,
        currency: asset.currency,
        unit_label: asset.unit_label,
        units_or_balance: principal,
        current_value: Some(principal),
        current_value_eur,
        net_contributions_eur,
        invested_eur,
        realized_return_eur,
        unrealized_return_eur,
        total_return_eur,
        income_eur,
        valuation_date: None,
    }
}

#[derive(Debug, Default, PartialEq)]
struct CashFold {
    balance: Decimal,
    balance_eur_at_events: Decimal,
    net_contributions_eur: Decimal,
    invested_eur: Decimal,
    income_eur: Decimal,
    realized_fx_eur: Decimal,
    last_snapshot_date: Option<DateTime<Utc>>,
}

enum CashEvent<'a> {
    Snapshot(&'a AssetBalanceSnapshot),
    Transaction(&'a AssetTransaction),
    Interest(&'a LinkedAssetInterest),
}

impl CashEvent<'_> {
    fn date(&self) -> DateTime<Utc> {
        match self {
            Self::Snapshot(snapshot) => snapshot.date,
            Self::Transaction(transaction) => transaction.date,
            Self::Interest(interest) => interest.date,
        }
    }

    fn sort_order(&self) -> u8 {
        match self {
            Self::Snapshot(snapshot) => match snapshot.kind {
                AssetBalanceSnapshotKind::Opening => 0,
                AssetBalanceSnapshotKind::Reconciliation => 3,
            },
            Self::Transaction(transaction) => match transaction.kind {
                AssetTransactionKind::Deposit | AssetTransactionKind::PrincipalAdvance => 1,
                AssetTransactionKind::Withdrawal | AssetTransactionKind::PrincipalRepayment => 2,
            },
            Self::Interest(_) => 1,
        }
    }

    fn created_at(&self) -> DateTime<Utc> {
        match self {
            Self::Snapshot(snapshot) => snapshot.created_at,
            Self::Transaction(transaction) => transaction.created_at,
            Self::Interest(interest) => interest.date,
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Snapshot(snapshot) => &snapshot.id,
            Self::Transaction(transaction) => &transaction.id,
            Self::Interest(interest) => &interest.id,
        }
    }
}

fn fold_cash_events(
    transactions: &[AssetTransaction],
    snapshots: &[AssetBalanceSnapshot],
    interest: &[LinkedAssetInterest],
    as_of: DateTime<Utc>,
) -> CashFold {
    let mut events: Vec<CashEvent<'_>> = snapshots
        .iter()
        .map(CashEvent::Snapshot)
        .chain(transactions.iter().map(CashEvent::Transaction))
        .chain(interest.iter().map(CashEvent::Interest))
        .filter(|event| event.date() <= as_of)
        .collect();
    events.sort_by(|left, right| {
        left.date()
            .cmp(&right.date())
            .then(left.sort_order().cmp(&right.sort_order()))
            .then(left.created_at().cmp(&right.created_at()))
            .then(left.id().cmp(right.id()))
    });

    let mut folded = CashFold::default();
    for event in events {
        match event {
            CashEvent::Snapshot(snapshot) => {
                if snapshot.kind == AssetBalanceSnapshotKind::Opening {
                    // Opening is the performance baseline: activity before it is intentionally ignored.
                    folded.balance = snapshot.balance;
                    folded.balance_eur_at_events = snapshot.balance_eur;
                    folded.net_contributions_eur = snapshot.balance_eur;
                    folded.invested_eur = snapshot.balance_eur;
                    folded.income_eur = Decimal::ZERO;
                } else {
                    // Only the unexplained native balance delta is capital. Revaluing the existing
                    // balance at the snapshot's FX rate must not erase accumulated FX return.
                    let native_delta = snapshot.balance - folded.balance;
                    let snapshot_rate = if snapshot.balance > Decimal::ZERO {
                        snapshot.balance_eur / snapshot.balance
                    } else if folded.balance > Decimal::ZERO {
                        folded.balance_eur_at_events / folded.balance
                    } else {
                        Decimal::ZERO
                    };
                    let reconciliation_delta_eur = native_delta * snapshot_rate;
                    let removed_cost_basis =
                        if native_delta < Decimal::ZERO && folded.balance > Decimal::ZERO {
                            folded.balance_eur_at_events * -native_delta / folded.balance
                        } else {
                            Decimal::ZERO
                        };
                    folded.balance = snapshot.balance;
                    if native_delta < Decimal::ZERO {
                        folded.balance_eur_at_events -= removed_cost_basis;
                        folded.realized_fx_eur += -reconciliation_delta_eur - removed_cost_basis;
                    } else {
                        folded.balance_eur_at_events += reconciliation_delta_eur;
                    }
                    folded.net_contributions_eur += reconciliation_delta_eur;
                    if reconciliation_delta_eur > Decimal::ZERO {
                        folded.invested_eur += reconciliation_delta_eur;
                    }
                }
                folded.last_snapshot_date = Some(snapshot.date);
            }
            CashEvent::Transaction(transaction) => match transaction.kind {
                AssetTransactionKind::Deposit => {
                    folded.balance += transaction.amount;
                    folded.balance_eur_at_events += transaction.amount_eur;
                    folded.net_contributions_eur += transaction.amount_eur;
                    folded.invested_eur += transaction.amount_eur;
                }
                AssetTransactionKind::Withdrawal => {
                    let removed_cost_basis = if folded.balance > Decimal::ZERO {
                        folded.balance_eur_at_events * transaction.amount / folded.balance
                    } else {
                        Decimal::ZERO
                    };
                    folded.balance -= transaction.amount;
                    folded.balance_eur_at_events -= removed_cost_basis;
                    folded.net_contributions_eur -= transaction.amount_eur;
                    folded.realized_fx_eur += transaction.amount_eur - removed_cost_basis;
                }
                AssetTransactionKind::PrincipalAdvance
                | AssetTransactionKind::PrincipalRepayment => {}
            },
            CashEvent::Interest(interest) => {
                folded.balance += interest.amount;
                folded.balance_eur_at_events += interest.amount_eur;
                folded.income_eur += interest.amount_eur;
            }
        }
    }
    folded
}

async fn resolve_eur(
    native_amount: Decimal,
    explicit_eur_amount: Option<Decimal>,
    date: DateTime<Utc>,
    currency: &str,
) -> anyhow::Result<Decimal> {
    if currency == "EUR" {
        if explicit_eur_amount.is_some_and(|amount| amount != native_amount) {
            bail!("EUR equivalent must equal the native EUR amount");
        }
        return Ok(native_amount);
    }
    if native_amount == Decimal::ZERO {
        if explicit_eur_amount.is_some_and(|amount| amount != Decimal::ZERO) {
            bail!("EUR equivalent must be zero when the native amount is zero");
        }
        return Ok(Decimal::ZERO);
    }
    let resolved = match explicit_eur_amount {
        Some(amount_eur) => Ok(amount_eur),
        None => convert_amount(native_amount, &date.date_naive(), currency, "EUR").await,
    }?;
    if native_amount > Decimal::ZERO && resolved <= Decimal::ZERO {
        bail!("EUR equivalent must be positive when the native amount is positive");
    }
    Ok(resolved)
}

async fn resolve_trade_fees_eur(
    fees: Decimal,
    native_price: Decimal,
    explicit_eur_price: Option<Decimal>,
    date: DateTime<Utc>,
    currency: &str,
) -> anyhow::Result<Decimal> {
    if fees == Decimal::ZERO {
        return Ok(Decimal::ZERO);
    }
    if currency == "EUR" {
        return Ok(fees);
    }
    if let Some(eur_price) = explicit_eur_price {
        if native_price > Decimal::ZERO {
            return Ok(fees * eur_price / native_price);
        }
    }
    resolve_eur(fees, None, date, currency).await
}

async fn try_convert_current(
    native_amount: Decimal,
    as_of: DateTime<Utc>,
    currency: &str,
    asset_id: &str,
) -> Option<Decimal> {
    if native_amount == Decimal::ZERO {
        return Some(Decimal::ZERO);
    }
    match convert_amount(native_amount, &as_of.date_naive(), currency, "EUR").await {
        Ok(amount) => Some(amount),
        Err(error) => {
            log::warn!("Unable to value custom asset {asset_id} in EUR: {error}");
            None
        }
    }
}

async fn require_asset(asset_id: &str) -> anyhow::Result<Asset> {
    asset_queries::get_asset(asset_id)
        .await?
        .with_context(|| format!("asset not found: {asset_id}"))
}

async fn require_active_asset(asset_id: &str) -> anyhow::Result<Asset> {
    let asset = require_asset(asset_id).await?;
    if asset.archived_at.is_some() {
        bail!("asset is archived: {asset_id}");
    }
    Ok(asset)
}

fn ensure_trade_asset(asset: &Asset) -> anyhow::Result<()> {
    if !asset.asset_class.supports_trades() {
        bail!(
            "{} does not support trades or valuations",
            asset.asset_class
        );
    }
    Ok(())
}

fn ensure_cash_account(asset: &Asset) -> anyhow::Result<()> {
    if asset.asset_class != AssetClass::CashAccount {
        bail!("{} does not support balance snapshots", asset.asset_class);
    }
    Ok(())
}

fn ensure_interest_asset(asset: &Asset) -> anyhow::Result<()> {
    if !asset.asset_class.supports_interest() {
        bail!("{} does not support linked interest", asset.asset_class);
    }
    Ok(())
}

fn ensure_interest_principal(asset: &Asset, principal: Option<&str>) -> anyhow::Result<()> {
    let compatible = match asset.asset_class {
        AssetClass::CashAccount => principal == Some("Cash"),
        AssetClass::PrivateDebt => principal == Some("PrivateDebt"),
        AssetClass::PhysicalGold | AssetClass::RealEstate => false,
    };
    if !compatible {
        bail!(
            "interest principal is incompatible with {}",
            asset.asset_class
        );
    }
    Ok(())
}

fn ensure_transaction_kind(asset: &Asset, kind: AssetTransactionKind) -> anyhow::Result<()> {
    let compatible = matches!(
        (asset.asset_class, kind),
        (
            AssetClass::CashAccount,
            AssetTransactionKind::Deposit | AssetTransactionKind::Withdrawal
        ) | (
            AssetClass::PrivateDebt,
            AssetTransactionKind::PrincipalAdvance | AssetTransactionKind::PrincipalRepayment
        )
    );
    if !compatible {
        bail!("{kind:?} is not valid for {}", asset.asset_class);
    }
    Ok(())
}

fn ensure_currency(asset: &Asset, currency: &str) -> anyhow::Result<()> {
    if asset.currency != currency {
        bail!(
            "activity currency {currency} does not match asset currency {}",
            asset.currency
        );
    }
    Ok(())
}

async fn ensure_sell_inventory(
    asset_id: &str,
    date: DateTime<Utc>,
    units: Decimal,
    direction: AssetTradeDirection,
    excluded_trade_id: Option<&str>,
) -> anyhow::Result<()> {
    if direction != AssetTradeDirection::Sell {
        return Ok(());
    }

    let available = asset_queries::list_asset_trades(asset_id)
        .await?
        .into_iter()
        .filter(|trade| trade.date <= date && Some(trade.id.as_str()) != excluded_trade_id)
        .fold(Decimal::ZERO, |inventory, trade| match trade.direction {
            AssetTradeDirection::Buy => inventory + trade.units,
            AssetTradeDirection::Sell => inventory - trade.units,
        });
    if units > available {
        bail!("cannot sell {units} units; only {available} are held at {date}");
    }
    Ok(())
}

async fn ensure_available_balance(
    asset: &Asset,
    date: DateTime<Utc>,
    kind: AssetTransactionKind,
    amount: Decimal,
    excluded_transaction_id: Option<&str>,
) -> anyhow::Result<()> {
    let decreases_balance = matches!(
        (asset.asset_class, kind),
        (AssetClass::CashAccount, AssetTransactionKind::Withdrawal)
            | (
                AssetClass::PrivateDebt,
                AssetTransactionKind::PrincipalRepayment
            )
    );
    if !decreases_balance {
        return Ok(());
    }

    let detail = asset_queries::get_asset_detail(&asset.id)
        .await?
        .context("asset disappeared while validating its balance")?;
    let available = match detail.activity {
        AssetActivity::CashAccount(mut activity) => {
            activity
                .transactions
                .retain(|item| Some(item.id.as_str()) != excluded_transaction_id);
            fold_cash_events(
                &activity.transactions,
                &activity.balance_snapshots,
                &activity.interest,
                date,
            )
            .balance
        }
        AssetActivity::PrivateDebt(activity) => activity
            .transactions
            .into_iter()
            .filter(|item| item.date <= date && Some(item.id.as_str()) != excluded_transaction_id)
            .fold(Decimal::ZERO, |principal, item| match item.kind {
                AssetTransactionKind::PrincipalAdvance => principal + item.amount,
                AssetTransactionKind::PrincipalRepayment => principal - item.amount,
                AssetTransactionKind::Deposit | AssetTransactionKind::Withdrawal => principal,
            }),
        _ => Decimal::ZERO,
    };
    if amount > available {
        bail!("cannot record {kind:?} of {amount}; only {available} is available at {date}");
    }
    Ok(())
}

fn detail_has_activity(detail: &AssetDetailRecord) -> bool {
    match &detail.activity {
        AssetActivity::PhysicalGold(activity) | AssetActivity::RealEstate(activity) => {
            !activity.trades.is_empty() || !activity.valuations.is_empty()
        }
        AssetActivity::PrivateDebt(activity) => {
            !activity.transactions.is_empty() || !activity.interest.is_empty()
        }
        AssetActivity::CashAccount(activity) => {
            !activity.transactions.is_empty()
                || !activity.balance_snapshots.is_empty()
                || !activity.interest.is_empty()
        }
    }
}

fn detail_has_future_activity(detail: &AssetDetailRecord, now: DateTime<Utc>) -> bool {
    match &detail.activity {
        AssetActivity::PhysicalGold(activity) | AssetActivity::RealEstate(activity) => {
            activity.trades.iter().any(|item| item.date > now)
                || activity.valuations.iter().any(|item| item.date > now)
        }
        AssetActivity::PrivateDebt(activity) => {
            activity.transactions.iter().any(|item| item.date > now)
                || activity.interest.iter().any(|item| item.date > now)
        }
        AssetActivity::CashAccount(activity) => {
            activity.transactions.iter().any(|item| item.date > now)
                || activity
                    .balance_snapshots
                    .iter()
                    .any(|item| item.date > now)
                || activity.interest.iter().any(|item| item.date > now)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rust_decimal_macros::dec;

    use super::*;
    use crate::database::models::asset::{AssetBalanceSnapshotKind, InterestOrigin, TaxTreatment};

    fn date(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, day, 12, 0, 0).unwrap()
    }

    fn transaction(
        id: &str,
        day: u32,
        kind: AssetTransactionKind,
        amount: Decimal,
    ) -> AssetTransaction {
        AssetTransaction {
            id: id.to_string(),
            asset_id: "cash".to_string(),
            date: date(day),
            kind,
            amount,
            amount_eur: amount,
            currency: "EUR".to_string(),
            note: None,
            created_at: date(day),
        }
    }

    fn snapshot(
        id: &str,
        day: u32,
        kind: AssetBalanceSnapshotKind,
        balance: Decimal,
    ) -> AssetBalanceSnapshot {
        AssetBalanceSnapshot {
            id: id.to_string(),
            asset_id: "cash".to_string(),
            date: date(day),
            kind,
            balance,
            balance_eur: balance,
            note: None,
            created_at: date(day),
        }
    }

    fn interest(day: u32, amount: Decimal) -> LinkedAssetInterest {
        LinkedAssetInterest {
            id: format!("interest-{day}"),
            asset_id: Some("cash".to_string()),
            date: date(day),
            amount,
            broker: None,
            principal: None,
            currency: "EUR".to_string(),
            amount_eur: amount,
            withholding_tax: None,
            withholding_tax_currency: None,
            tax_treatment: TaxTreatment::Excluded,
            origin: InterestOrigin::Manual,
        }
    }

    #[test]
    fn opening_snapshot_discards_earlier_activity_and_folds_later_events() {
        let transactions = vec![
            transaction("old", 1, AssetTransactionKind::Deposit, dec!(100)),
            transaction("deposit", 3, AssetTransactionKind::Deposit, dec!(25)),
            transaction("withdrawal", 5, AssetTransactionKind::Withdrawal, dec!(10)),
        ];
        let snapshots = vec![snapshot(
            "opening",
            2,
            AssetBalanceSnapshotKind::Opening,
            dec!(50),
        )];
        let interest = vec![interest(4, dec!(2))];

        let folded = fold_cash_events(&transactions, &snapshots, &interest, date(10));

        assert_eq!(folded.balance, dec!(67));
        assert_eq!(folded.net_contributions_eur, dec!(65));
        assert_eq!(folded.income_eur, dec!(2));
    }

    #[test]
    fn reconciliation_delta_is_capital_neutral() {
        let transactions = vec![transaction(
            "deposit",
            2,
            AssetTransactionKind::Deposit,
            dec!(20),
        )];
        let snapshots = vec![
            snapshot("opening", 1, AssetBalanceSnapshotKind::Opening, dec!(100)),
            snapshot(
                "reconciliation",
                3,
                AssetBalanceSnapshotKind::Reconciliation,
                dec!(125),
            ),
        ];

        let folded = fold_cash_events(&transactions, &snapshots, &[], date(10));

        assert_eq!(folded.balance, dec!(125));
        assert_eq!(folded.net_contributions_eur, dec!(125));
        assert_eq!(folded.balance - folded.net_contributions_eur, dec!(0));
    }

    #[test]
    fn reconciliation_to_zero_removes_the_existing_eur_basis() {
        let snapshots = vec![
            snapshot("opening", 1, AssetBalanceSnapshotKind::Opening, dec!(100)),
            snapshot(
                "reconciliation",
                2,
                AssetBalanceSnapshotKind::Reconciliation,
                dec!(0),
            ),
        ];

        let folded = fold_cash_events(&[], &snapshots, &[], date(10));

        assert_eq!(folded.balance, dec!(0));
        assert_eq!(folded.net_contributions_eur, dec!(0));
    }

    #[test]
    fn withdrawal_realizes_foreign_exchange_return() {
        let mut opening = snapshot("opening", 1, AssetBalanceSnapshotKind::Opening, dec!(100));
        opening.balance_eur = dec!(90);
        let mut withdrawal =
            transaction("withdrawal", 2, AssetTransactionKind::Withdrawal, dec!(100));
        withdrawal.amount_eur = dec!(95);

        let folded = fold_cash_events(&[withdrawal], &[opening], &[], date(10));

        assert_eq!(folded.balance, dec!(0));
        assert_eq!(folded.net_contributions_eur, dec!(-5));
        assert_eq!(folded.realized_fx_eur, dec!(5));
    }

    #[tokio::test]
    async fn explicit_eur_values_must_match_eur_native_values() {
        let error = resolve_eur(dec!(100), Some(dec!(1)), date(1), "EUR")
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("EUR equivalent must equal the native EUR amount"));
    }

    #[tokio::test]
    async fn zero_native_values_reject_nonzero_eur_equivalents() {
        let error = resolve_eur(dec!(0), Some(dec!(1)), date(1), "USD")
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("EUR equivalent must be zero when the native amount is zero"));
    }

    #[test]
    fn weighted_average_uses_only_positive_converted_balances() {
        let accounts = vec![
            cash_summary("a", dec!(100), Some(dec!(100)), dec!(2)),
            cash_summary("b", dec!(300), Some(dec!(300)), dec!(4)),
            cash_summary("zero", dec!(0), Some(dec!(0)), dec!(99)),
            cash_summary("missing", dec!(50), None, dec!(10)),
        ];

        let summary = weighted_cash_interest_summary(accounts);

        assert_eq!(summary.average_interest_rate_percent, Some(dec!(3.5)));
        assert_eq!(summary.eligible_balance_eur, dec!(400));
        assert_eq!(summary.unconverted_count, 1);
    }

    #[test]
    fn weighted_average_is_none_without_eligible_balance() {
        let accounts = vec![cash_summary("a", dec!(10), None, dec!(2))];

        let summary = weighted_cash_interest_summary(accounts);

        assert_eq!(summary.average_interest_rate_percent, None);
        assert_eq!(summary.eligible_balance_eur, dec!(0));
        assert_eq!(summary.unconverted_count, 1);
    }

    fn cash_summary(
        id: &str,
        native_balance: Decimal,
        eur_balance: Option<Decimal>,
        rate: Decimal,
    ) -> CashAccountSummary {
        CashAccountSummary {
            asset_id: id.to_string(),
            name: id.to_string(),
            currency: "EUR".to_string(),
            current_balance: native_balance,
            current_balance_eur: eur_balance,
            current_interest_rate_percent: rate,
            interest_rate_updated_at: None,
        }
    }
}
