use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use super::handlers::{
    assets,
    auth::{self, LoginRequestData},
    market, performance, portfolio, taxation, timeline, trades,
};
use crate::{
    api::errors::{ErrorDetails, ErrorResponse},
    database::models::{
        asset::{
            Asset, AssetActivity, AssetBalanceSnapshot, AssetBalanceSnapshotKind, AssetClass,
            AssetDetailRecord, AssetListRecord, AssetTrade, AssetTradeDirection, AssetTransaction,
            AssetTransactionKind, AssetValuation, AssetValuationSource, CashAccountActivity,
            CashAccountSummary, CreateAssetBalanceSnapshotRequest, CreateAssetRequest,
            CreateAssetTradeRequest, CreateAssetTransactionRequest, CreateAssetValuationRequest,
            CreateLinkedInterestRequest, CustomAssetHolding, InterestOrigin, LinkedAssetInterest,
            PrivateDebtActivity, TaxTreatment, TradeAssetActivity,
            UpdateAssetBalanceSnapshotRequest, UpdateAssetRequest, UpdateAssetTradeRequest,
            UpdateAssetTransactionRequest, UpdateAssetValuationRequest,
            WeightedCashInterestSummary,
        },
        dividend::Dividend,
        fx_conversion::FxConversion,
        instrument::Instrument,
        interest::{InterestPayment, InterestRecord},
        performance::PerformanceSignal,
        position::PositionWithName,
        position::PositionWithValueAndAllocation,
        trade::{MonthlyNetInflow, Trade},
    },
    services::{
        events::{EventType, PortfolioEvent, TradeDirection},
        performance::{BuyIn, PortfolioPerformance, PositionPerformance},
        portfolio::PortfolioOverview,
        taxation::{AnnualTaxableAmounts, FxWac, SecWac, TaxationReport, TransactionTaxImpact},
    },
};

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "api_token",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("API_TOKEN")
                        .description(Some(
                            "Set the `API_TOKEN` environment variable and send `Authorization: Bearer <token>`. \
                             Browser sessions from POST /api/login are also accepted.",
                        ))
                        .build(),
                ),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Doughbox API",
        description = "REST API for portfolio allocations, custom assets, trades, performance, and Austrian tax reports.\n\n\
            Authenticate with a session cookie from `POST /api/login` or `Authorization: Bearer $API_TOKEN`.",
        version = "0.1.0"
    ),
    paths(
        auth::login,
        auth::logout,
        auth::auth_state,
        assets::list_assets,
        assets::create_asset,
        assets::get_asset,
        assets::update_asset,
        assets::archive_asset,
        assets::restore_asset,
        assets::holdings,
        assets::cash_summary,
        assets::create_trade,
        assets::update_trade,
        assets::delete_trade,
        assets::create_valuation,
        assets::update_valuation,
        assets::delete_valuation,
        assets::create_transaction,
        assets::update_transaction,
        assets::delete_transaction,
        assets::create_balance_snapshot,
        assets::update_balance_snapshot,
        assets::delete_balance_snapshot,
        assets::create_interest,
        assets::update_interest,
        assets::link_interest,
        assets::unlink_interest,
        portfolio::portfolio,
        portfolio::positions,
        timeline::timeline,
        timeline::net_inflow,
        trades::trades,
        trades::buy_ins,
        trades::dividends,
        trades::interest,
        trades::fx_conversions,
        performance::live_performance,
        performance::performance_overview,
        performance::past_performance,
        market::instruments,
        market::instrument,
        market::brokers,
        taxation::taxation,
        taxation::taxation_detailed,
        taxation::taxation_transactions,
    ),
    components(schemas(
        LoginRequestData,
        ErrorResponse,
        ErrorDetails,
        PortfolioOverview,
        PositionWithValueAndAllocation,
        PositionWithName,
        PortfolioEvent,
        EventType,
        TradeDirection,
        Trade,
        MonthlyNetInflow,
        BuyIn,
        Dividend,
        InterestPayment,
        InterestRecord,
        FxConversion,
        PortfolioPerformance,
        PositionPerformance,
        PerformanceSignal,
        Instrument,
        TaxationReport,
        AnnualTaxableAmounts,
        SecWac,
        FxWac,
        TransactionTaxImpact,
        Asset,
        AssetClass,
        TaxTreatment,
        InterestOrigin,
        AssetListRecord,
        AssetDetailRecord,
        AssetActivity,
        TradeAssetActivity,
        CashAccountActivity,
        PrivateDebtActivity,
        AssetTrade,
        AssetTradeDirection,
        AssetValuation,
        AssetValuationSource,
        AssetTransaction,
        AssetTransactionKind,
        AssetBalanceSnapshot,
        AssetBalanceSnapshotKind,
        LinkedAssetInterest,
        CustomAssetHolding,
        CashAccountSummary,
        WeightedCashInterestSummary,
        CreateAssetRequest,
        UpdateAssetRequest,
        CreateAssetTradeRequest,
        UpdateAssetTradeRequest,
        CreateAssetValuationRequest,
        UpdateAssetValuationRequest,
        CreateAssetTransactionRequest,
        UpdateAssetTransactionRequest,
        CreateAssetBalanceSnapshotRequest,
        UpdateAssetBalanceSnapshotRequest,
        CreateLinkedInterestRequest,
    )),
    tags(
        (name = "auth", description = "Session login and logout"),
        (name = "portfolio", description = "Allocations, holdings, and timeline"),
        (name = "trades", description = "Trades, buy-ins, dividends, interest, and FX"),
        (name = "performance", description = "Live and precomputed performance"),
        (name = "market", description = "Instruments and brokers"),
        (name = "taxation", description = "Austrian capital-gains tax reports"),
        (name = "assets", description = "Custom assets, balances, valuations, and cash accounts"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;
