use std::{fmt, str::FromStr};

use anyhow::bail;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use typeshare::typeshare;
use utoipa::ToSchema;

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AssetClass {
    PhysicalGold,
    RealEstate,
    PrivateDebt,
    CashAccount,
}

impl AssetClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PhysicalGold => "PhysicalGold",
            Self::RealEstate => "RealEstate",
            Self::PrivateDebt => "PrivateDebt",
            Self::CashAccount => "CashAccount",
        }
    }

    pub fn supports_trades(self) -> bool {
        matches!(self, Self::PhysicalGold | Self::RealEstate)
    }

    pub fn supports_transactions(self) -> bool {
        matches!(self, Self::PrivateDebt | Self::CashAccount)
    }

    pub fn supports_interest(self) -> bool {
        matches!(self, Self::PrivateDebt | Self::CashAccount)
    }
}

impl fmt::Display for AssetClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AssetClass {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "PhysicalGold" => Ok(Self::PhysicalGold),
            "RealEstate" => Ok(Self::RealEstate),
            "PrivateDebt" => Ok(Self::PrivateDebt),
            "CashAccount" => Ok(Self::CashAccount),
            _ => bail!("unknown asset class: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TaxTreatment {
    Included,
    Excluded,
}

impl FromStr for TaxTreatment {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Included" => Ok(Self::Included),
            "Excluded" => Ok(Self::Excluded),
            _ => bail!("unknown tax treatment: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum InterestOrigin {
    Imported,
    Manual,
}

impl FromStr for InterestOrigin {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Imported" => Ok(Self::Imported),
            "Manual" => Ok(Self::Manual),
            _ => bail!("unknown interest origin: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AssetTradeDirection {
    Buy,
    Sell,
}

impl AssetTradeDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "Buy",
            Self::Sell => "Sell",
        }
    }
}

impl FromStr for AssetTradeDirection {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Buy" => Ok(Self::Buy),
            "Sell" => Ok(Self::Sell),
            _ => bail!("unknown asset trade direction: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AssetValuationSource {
    Manual,
    Trade,
}

impl FromStr for AssetValuationSource {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Manual" => Ok(Self::Manual),
            "Trade" => Ok(Self::Trade),
            _ => bail!("unknown asset valuation source: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AssetTransactionKind {
    Deposit,
    Withdrawal,
    PrincipalAdvance,
    PrincipalRepayment,
}

impl AssetTransactionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deposit => "Deposit",
            Self::Withdrawal => "Withdrawal",
            Self::PrincipalAdvance => "PrincipalAdvance",
            Self::PrincipalRepayment => "PrincipalRepayment",
        }
    }
}

impl FromStr for AssetTransactionKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Deposit" => Ok(Self::Deposit),
            "Withdrawal" => Ok(Self::Withdrawal),
            "PrincipalAdvance" => Ok(Self::PrincipalAdvance),
            "PrincipalRepayment" => Ok(Self::PrincipalRepayment),
            _ => bail!("unknown asset transaction kind: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AssetBalanceSnapshotKind {
    Opening,
    Reconciliation,
}

impl AssetBalanceSnapshotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "Opening",
            Self::Reconciliation => "Reconciliation",
        }
    }
}

impl FromStr for AssetBalanceSnapshotKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Opening" => Ok(Self::Opening),
            "Reconciliation" => Ok(Self::Reconciliation),
            _ => bail!("unknown asset balance snapshot kind: {value}"),
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateAssetRequest {
    pub name: String,
    pub asset_class: AssetClass,
    pub currency: String,
    pub unit_label: String,
    pub current_interest_rate_percent: Option<Decimal>,
}

impl CreateAssetRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_nonempty("name", &self.name)?;
        validate_nonempty("unit_label", &self.unit_label)?;
        validate_currency(&self.currency)?;
        validate_unit_label(self.asset_class, &self.currency, &self.unit_label)?;
        validate_interest_rate(self.asset_class, self.current_interest_rate_percent)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateAssetRequest {
    pub name: String,
    pub currency: String,
    pub unit_label: String,
    pub current_interest_rate_percent: Option<Decimal>,
}

impl UpdateAssetRequest {
    pub fn validate_for(&self, asset_class: AssetClass) -> anyhow::Result<()> {
        validate_nonempty("name", &self.name)?;
        validate_nonempty("unit_label", &self.unit_label)?;
        validate_currency(&self.currency)?;
        validate_unit_label(asset_class, &self.currency, &self.unit_label)?;
        validate_interest_rate(asset_class, self.current_interest_rate_percent)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub asset_class: AssetClass,
    pub currency: String,
    pub unit_label: String,
    pub current_interest_rate_percent: Option<Decimal>,
    pub interest_rate_updated_at: Option<DateTime<Utc>>,
    pub tax_treatment: TaxTreatment,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetListRecord {
    pub id: String,
    pub name: String,
    pub asset_class: AssetClass,
    pub currency: String,
    pub unit_label: String,
    pub current_interest_rate_percent: Option<Decimal>,
    pub interest_rate_updated_at: Option<DateTime<Utc>>,
    pub tax_treatment: TaxTreatment,
    pub archived_at: Option<DateTime<Utc>>,
}

impl From<Asset> for AssetListRecord {
    fn from(asset: Asset) -> Self {
        Self {
            id: asset.id,
            name: asset.name,
            asset_class: asset.asset_class,
            currency: asset.currency,
            unit_label: asset.unit_label,
            current_interest_rate_percent: asset.current_interest_rate_percent,
            interest_rate_updated_at: asset.interest_rate_updated_at,
            tax_treatment: asset.tax_treatment,
            archived_at: asset.archived_at,
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateAssetTradeRequest {
    pub date: DateTime<Utc>,
    pub units: Decimal,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Option<Decimal>,
    pub direction: AssetTradeDirection,
    pub currency: String,
    pub broker: String,
    #[serde(default)]
    pub fees: Decimal,
    pub note: Option<String>,
}

impl CreateAssetTradeRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_positive("units", self.units)?;
        validate_nonnegative("price_per_unit", self.price_per_unit)?;
        validate_optional_nonnegative("eur_price_per_unit", self.eur_price_per_unit)?;
        validate_currency(&self.currency)?;
        validate_nonempty("broker", &self.broker)?;
        validate_nonnegative("fees", self.fees)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateAssetTradeRequest {
    pub date: DateTime<Utc>,
    pub units: Decimal,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Option<Decimal>,
    pub direction: AssetTradeDirection,
    pub currency: String,
    pub broker: String,
    #[serde(default)]
    pub fees: Decimal,
    pub note: Option<String>,
}

impl UpdateAssetTradeRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_positive("units", self.units)?;
        validate_nonnegative("price_per_unit", self.price_per_unit)?;
        validate_optional_nonnegative("eur_price_per_unit", self.eur_price_per_unit)?;
        validate_currency(&self.currency)?;
        validate_nonempty("broker", &self.broker)?;
        validate_nonnegative("fees", self.fees)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetTrade {
    pub id: String,
    pub asset_id: String,
    pub date: DateTime<Utc>,
    pub units: Decimal,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Decimal,
    pub direction: AssetTradeDirection,
    pub currency: String,
    pub broker: String,
    pub fees: Decimal,
    pub fees_eur: Decimal,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateAssetValuationRequest {
    pub date: DateTime<Utc>,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Option<Decimal>,
    pub currency: String,
}

impl CreateAssetValuationRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_nonnegative("price_per_unit", self.price_per_unit)?;
        validate_optional_nonnegative("eur_price_per_unit", self.eur_price_per_unit)?;
        validate_currency(&self.currency)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateAssetValuationRequest {
    pub date: DateTime<Utc>,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Option<Decimal>,
    pub currency: String,
}

impl UpdateAssetValuationRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_nonnegative("price_per_unit", self.price_per_unit)?;
        validate_optional_nonnegative("eur_price_per_unit", self.eur_price_per_unit)?;
        validate_currency(&self.currency)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetValuation {
    pub id: String,
    pub asset_id: String,
    pub date: DateTime<Utc>,
    pub price_per_unit: Decimal,
    pub eur_price_per_unit: Decimal,
    pub currency: String,
    pub source: AssetValuationSource,
    pub created_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateAssetTransactionRequest {
    pub date: DateTime<Utc>,
    pub kind: AssetTransactionKind,
    pub amount: Decimal,
    pub amount_eur: Option<Decimal>,
    pub currency: String,
    pub note: Option<String>,
}

impl CreateAssetTransactionRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_positive("amount", self.amount)?;
        validate_optional_nonnegative("amount_eur", self.amount_eur)?;
        validate_currency(&self.currency)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateAssetTransactionRequest {
    pub date: DateTime<Utc>,
    pub kind: AssetTransactionKind,
    pub amount: Decimal,
    pub amount_eur: Option<Decimal>,
    pub currency: String,
    pub note: Option<String>,
}

impl UpdateAssetTransactionRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_positive("amount", self.amount)?;
        validate_optional_nonnegative("amount_eur", self.amount_eur)?;
        validate_currency(&self.currency)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetTransaction {
    pub id: String,
    pub asset_id: String,
    pub date: DateTime<Utc>,
    pub kind: AssetTransactionKind,
    pub amount: Decimal,
    pub amount_eur: Decimal,
    pub currency: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateAssetBalanceSnapshotRequest {
    pub date: DateTime<Utc>,
    pub kind: AssetBalanceSnapshotKind,
    pub balance: Decimal,
    pub balance_eur: Option<Decimal>,
    pub note: Option<String>,
}

impl CreateAssetBalanceSnapshotRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_nonnegative("balance", self.balance)?;
        validate_optional_nonnegative("balance_eur", self.balance_eur)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateAssetBalanceSnapshotRequest {
    pub date: DateTime<Utc>,
    pub kind: AssetBalanceSnapshotKind,
    pub balance: Decimal,
    pub balance_eur: Option<Decimal>,
    pub note: Option<String>,
}

impl UpdateAssetBalanceSnapshotRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_nonnegative("balance", self.balance)?;
        validate_optional_nonnegative("balance_eur", self.balance_eur)
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetBalanceSnapshot {
    pub id: String,
    pub asset_id: String,
    pub date: DateTime<Utc>,
    pub kind: AssetBalanceSnapshotKind,
    pub balance: Decimal,
    pub balance_eur: Decimal,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LinkedAssetInterest {
    pub id: String,
    pub asset_id: Option<String>,
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: Option<String>,
    pub principal: Option<String>,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Option<Decimal>,
    pub withholding_tax_currency: Option<String>,
    pub tax_treatment: TaxTreatment,
    pub origin: InterestOrigin,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateLinkedInterestRequest {
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub amount_eur: Option<Decimal>,
    pub currency: String,
    pub broker: Option<String>,
    pub principal: Option<String>,
    pub withholding_tax: Option<Decimal>,
    pub withholding_tax_currency: Option<String>,
}

impl CreateLinkedInterestRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_positive("amount", self.amount)?;
        validate_optional_nonnegative("amount_eur", self.amount_eur)?;
        validate_optional_nonnegative("withholding_tax", self.withholding_tax)?;
        validate_currency(&self.currency)?;
        if let Some(currency) = &self.withholding_tax_currency {
            validate_currency(currency)?;
        }
        if self.withholding_tax.is_some_and(|tax| tax > Decimal::ZERO)
            && self.withholding_tax_currency.is_none()
        {
            bail!("withholding_tax_currency is required when withholding_tax is positive");
        }
        Ok(())
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TradeAssetActivity {
    pub trades: Vec<AssetTrade>,
    pub valuations: Vec<AssetValuation>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CashAccountActivity {
    pub transactions: Vec<AssetTransaction>,
    pub balance_snapshots: Vec<AssetBalanceSnapshot>,
    pub interest: Vec<LinkedAssetInterest>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PrivateDebtActivity {
    pub transactions: Vec<AssetTransaction>,
    pub interest: Vec<LinkedAssetInterest>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "asset_class", content = "activity")]
pub enum AssetActivity {
    PhysicalGold(TradeAssetActivity),
    RealEstate(TradeAssetActivity),
    PrivateDebt(PrivateDebtActivity),
    CashAccount(CashAccountActivity),
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssetDetailRecord {
    pub asset: Asset,
    pub activity: AssetActivity,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CustomAssetHolding {
    pub asset_id: String,
    pub name: String,
    pub asset_class: AssetClass,
    pub currency: String,
    pub unit_label: String,
    pub units_or_balance: Decimal,
    pub current_value: Option<Decimal>,
    pub current_value_eur: Option<Decimal>,
    pub net_contributions_eur: Decimal,
    pub invested_eur: Decimal,
    pub realized_return_eur: Decimal,
    pub unrealized_return_eur: Option<Decimal>,
    pub total_return_eur: Option<Decimal>,
    pub income_eur: Decimal,
    pub valuation_date: Option<DateTime<Utc>>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CustomAssetPortfolioSummary {
    pub current_value_eur: Decimal,
    pub invested_eur: Decimal,
    pub known_invested_eur: Decimal,
    pub total_return_eur: Decimal,
    #[typeshare(serialized_as = "number")]
    pub incomplete_asset_count: i64,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CashAccountSummary {
    pub asset_id: String,
    pub name: String,
    pub currency: String,
    pub current_balance: Decimal,
    pub current_balance_eur: Option<Decimal>,
    pub current_interest_rate_percent: Decimal,
    pub interest_rate_updated_at: Option<DateTime<Utc>>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WeightedCashInterestSummary {
    pub average_interest_rate_percent: Option<Decimal>,
    pub eligible_balance_eur: Decimal,
    #[typeshare(serialized_as = "number")]
    pub unconverted_count: i64,
    pub accounts: Vec<CashAccountSummary>,
}

pub fn validate_currency(currency: &str) -> anyhow::Result<()> {
    if currency.len() != 3
        || !currency
            .bytes()
            .all(|character| character.is_ascii_uppercase())
    {
        bail!("currency must be exactly three uppercase ASCII letters");
    }
    Ok(())
}

fn validate_interest_rate(
    asset_class: AssetClass,
    interest_rate: Option<Decimal>,
) -> anyhow::Result<()> {
    if asset_class == AssetClass::CashAccount && interest_rate.is_none() {
        bail!("current_interest_rate_percent is required for CashAccount");
    }
    if asset_class != AssetClass::CashAccount && interest_rate.is_some() {
        bail!("current_interest_rate_percent is only valid for CashAccount");
    }
    validate_optional_range(
        "current_interest_rate_percent",
        interest_rate,
        Decimal::ZERO,
        Decimal::ONE_HUNDRED,
    )
}

fn validate_unit_label(
    asset_class: AssetClass,
    currency: &str,
    unit_label: &str,
) -> anyhow::Result<()> {
    if matches!(
        asset_class,
        AssetClass::CashAccount | AssetClass::PrivateDebt
    ) && unit_label != currency
    {
        bail!("unit_label must match currency for cash and private debt assets");
    }
    Ok(())
}

fn validate_nonempty(field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    Ok(())
}

fn validate_positive(field: &str, value: Decimal) -> anyhow::Result<()> {
    if value <= Decimal::ZERO {
        bail!("{field} must be positive");
    }
    Ok(())
}

fn validate_nonnegative(field: &str, value: Decimal) -> anyhow::Result<()> {
    if value < Decimal::ZERO {
        bail!("{field} must be nonnegative");
    }
    Ok(())
}

fn validate_optional_nonnegative(field: &str, value: Option<Decimal>) -> anyhow::Result<()> {
    if let Some(value) = value {
        validate_nonnegative(field, value)?;
    }
    Ok(())
}

fn validate_optional_range(
    field: &str,
    value: Option<Decimal>,
    minimum: Decimal,
    maximum: Decimal,
) -> anyhow::Result<()> {
    if let Some(value) = value {
        if value < minimum || value > maximum {
            bail!("{field} must be between {minimum} and {maximum}");
        }
    }
    Ok(())
}
