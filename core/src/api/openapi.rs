use utoipa::{
    openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme},
    Modify, OpenApi,
};

use super::handlers::{
    admin, api_tokens,
    assets,
    auth,
    data_imports, market, performance, portfolio, settings, statement_imports, taxation, timeline, trades,
    trading,
};
use crate::{
    api::errors::{ErrorDetails, ErrorResponse},
    database::models::{
        auth::{AuthState, AuthUserView, SessionStage, Role, AccountStatus, AccountLinkKind, AccountLinkView, SessionSummary, UserSummary, ApiTokenScope, ApiTokenSummary},
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
        instrument::{Instrument, InstrumentDetails},
        interest::{InterestPayment, InterestRecord},
        investment_activity::MonthlyInvestmentActivity,
        performance::{PerformanceComponent, PerformanceSignal},
        position::PositionWithName,
        position::PositionWithValueAndAllocation,
        statement_import::{StatementImportJob, StatementImportStatus, StatementImportsStatus},
        trade::Trade,
    },
    services::{
        auth::{challenges::{LoginRequest, VerificationRequest, ReauthRequest, CodeType}, credential_changes::{EmptyRequest, TotpStartResponse, TotpConfirmRequest, PendingTotpRequest, PasswordChangeRequest}, links::{InspectLinkRequest, RegisterRequest, PasswordResetRequest, RecoverRequest, IssuedAccountLink}},
        auth::{admin::{UserPatchRequest, UsersResponse, InviteSummary, InvitesResponse}, api_tokens::{CreateApiTokenRequest, IssuedApiToken, ApiTokensResponse}},
        calculations::{CalculationFrequency, CalculationSchedulerSettings, CalculationStatus},
        data_imports::{
            DataImportFrequency, DataImportSchedulerSettings, DataImportStatus, DataImportsStatus,
        },
        events::{EventType, PortfolioEvent, TradeDirection},
        market_data::prices::types::{
            PriceFeedStatus, PriceQuote, PriceSchedulerSettings, PricingStatus,
        },
        performance::{BuyIn, PortfolioPerformance, PositionPerformance, SecurityHistoryGap},
        portfolio::PortfolioOverview,
        shared::settings::{ApplicationSettings, ApplicationSettingsStatus, TelegramReportScheduleSettings, TelegramReportScheduleStatus},
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
                        .bearer_format("doughbox_api_")
                        .description(Some(
                            "Personal read/write token for the owner's private financial routes only; expires after 90 days. Never authorizes account administration, global controls or broker approval.",
                        ))
                        .build(),
                ),
            );
            components.add_security_scheme("browser_session", SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("doughbox_session"))));
        }
        for (path,item) in &mut openapi.paths.paths {
            if path == "/api/trading/proposals/{id}/approve" {
                if let Some(utoipa::openapi::RefOr::T(body)) = item.post.as_mut().and_then(|operation| operation.request_body.as_mut()) {
                    // Missing bytes are valid, but JSON null is not an approval object.
                    body.required = Some(utoipa::openapi::Required::False);
                }
            }
            let administrator = path.starts_with("/api/admin/") || path.starts_with("/api/data-imports")
                || path == "/api/prices/scheduler" || path == "/api/prices/refresh" || path.starts_with("/api/prices/feeds/");
            for operation in [&mut item.get,&mut item.post,&mut item.put,&mut item.patch,&mut item.delete,&mut item.head,&mut item.options].into_iter().flatten() {
                if administrator || path.starts_with("/api/trading") {
                    operation.security=Some(vec![SecurityRequirement::new("browser_session",Vec::<String>::new())]);
                    operation.description.get_or_insert_default().push_str(if administrator {
                        " Requires an administrator's full browser session; account-management mutations also require recent reauthentication."
                    } else { " Requires the owner's full browser session. Agent credentials do not authorize this endpoint." });
                } else if !path.starts_with("/api/auth/") {
                    operation.security=Some(vec![SecurityRequirement::new("browser_session",Vec::<String>::new()),SecurityRequirement::new("api_token",Vec::<String>::new())]);
                    operation.description.get_or_insert_default().push_str(" Reads and changes only the authenticated owner's portfolio. Personal read tokens permit GET; private writes require write scope.");
                }
            }
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Doughbox API",
        description = "REST API for portfolio allocations, custom assets, trades, buy-ins, performance, Austrian tax reports, instrument names and manual EUR price pins, automatic price feeds, and application settings.\n\n\
            Authenticate with username/password and mandatory TOTP or an unused recovery code. Password-only sessions are restricted. Browser mutations require same-origin Origin and X-CSRF-Token from GET /api/auth/state; cookie and bearer credentials must not be mixed. Account/admin mutations require browser credentials; sensitive actions require password plus factor proof within five minutes.",
        version = "0.1.0"
    ),
    paths(
        auth::login,
        auth::logout,
        auth::auth_state,
        auth::mfa_verify,
        auth::totp_start,
        auth::totp_confirm,
        auth::totp_cancel,
        auth::reauth,
        auth::change_password,
        auth::regenerate_recovery_codes,
        auth::sessions,
        auth::revoke_session,
        auth::inspect_link,
        auth::register,
        auth::reset_password,
        auth::recover_account,
        admin::list_users,
        admin::update_user,
        admin::revoke_user_sessions,
        admin::password_reset_link,
        admin::list_invites,
        admin::create_invite,
        admin::revoke_invite,
        api_tokens::list,
        api_tokens::create,
        api_tokens::revoke,
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
        timeline::investment_activity,
        trades::trades,
        trades::buy_ins,
        trades::dividends,
        trades::interest,
        trades::fx_conversions,
        performance::live_performance,
        performance::performance_overview,
        performance::past_performance,
        performance::get_performance_status,
        performance::refresh_performance,
        performance::save_performance_scheduler,
        market::instruments,
        market::instrument,
        market::rename,
        market::set_price,
        market::unpin_price,
        market::brokers,
        market::prices,
        market::save_scheduler,
        market::refresh_prices,
        market::refresh_price_feed,
        market::reset_price_feed,
        data_imports::get_data_imports,
        data_imports::refresh_data_import,
        data_imports::save_data_import_scheduler,
        statement_imports::get_statement_imports,
        statement_imports::upload_statement_imports,
        settings::get_settings,
        settings::get_admin_settings,
        settings::save_telegram,
        settings::save_settings,
        settings::save_telegram_scheduler,
        taxation::taxation,
        taxation::taxation_detailed,
        taxation::taxation_transactions,
        taxation::get_taxation_status,
        taxation::refresh_taxation,
        taxation::save_taxation_scheduler,
        trading::state,
        trading::connect,
        trading::disconnect,
        trading::token,
        trading::approve,
        trading::discard,
        trading::pies_list,
        trading::pie_detail,
        trading::broker_account,
        trading::submit_pie,
        trading::reconcile,
        trading::instruments,
    ),
    components(schemas(
        LoginRequest, VerificationRequest, ReauthRequest, CodeType, EmptyRequest,
        TotpStartResponse, TotpConfirmRequest, PendingTotpRequest, PasswordChangeRequest,
        AuthState, AuthUserView, SessionStage, Role, AccountStatus, AccountLinkKind, AccountLinkView, SessionSummary,
        auth::EnrollmentConfirmed, auth::RecoveryCodesResponse, auth::SessionsResponse,
        InspectLinkRequest, RegisterRequest, PasswordResetRequest, RecoverRequest, IssuedAccountLink,
        UserSummary, UserPatchRequest, UsersResponse, InviteSummary, InvitesResponse,
        ApiTokenScope, ApiTokenSummary, CreateApiTokenRequest, IssuedApiToken, ApiTokensResponse,
        ErrorResponse,
        ErrorDetails,
        PortfolioOverview,
        SecurityHistoryGap,
        PositionWithValueAndAllocation,
        PositionWithName,
        PortfolioEvent,
        EventType,
        TradeDirection,
        Trade,
        MonthlyInvestmentActivity,
        BuyIn,
        Dividend,
        InterestPayment,
        InterestRecord,
        FxConversion,
        PortfolioPerformance,
        PositionPerformance,
        PerformanceSignal,
        PerformanceComponent,
        Instrument,
        InstrumentDetails,
        market::RenameInstrumentRequest,
        market::ManualPriceRequest,
        PricingStatus,
        PriceFeedStatus,
        PriceSchedulerSettings,
        PriceQuote,
        CalculationStatus,
        CalculationFrequency,
        CalculationSchedulerSettings,
        DataImportStatus,
        DataImportsStatus,
        DataImportFrequency,
        DataImportSchedulerSettings,
        StatementImportJob,
        StatementImportStatus,
        StatementImportsStatus,
        statement_imports::StatementUploadRequest,
        ApplicationSettings,
        ApplicationSettingsStatus,
        TelegramReportScheduleSettings,
        TelegramReportScheduleStatus,
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
        crate::services::trading::Proposal,
        crate::services::trading::ProposalContent,
        crate::services::trading::pies::SubmitPieInput,
        crate::services::trading::pies::PieApprovalInput,
        crate::services::trading::pies::PieOperationInput,
        crate::services::trading::pies::PreparedPieMutation,
        crate::services::trading::pies::PieResult,
        crate::services::trading::pies::PieList,
        crate::services::trading::pies::PieDetail,
        crate::services::trading::account::BrokerAccountState,
        crate::services::trading::account::BrokerCash,
        crate::services::trading::account::BrokerPosition,
    )),
    tags(
        (name = "auth", description = "Username/password accounts, mandatory authenticator TOTP, durable browser sessions and offline recovery"),
        (name = "portfolio", description = "Allocations, holdings, and timeline"),
        (name = "trades", description = "Trades, buy-ins, dividends, interest, and FX"),
        (name = "performance", description = "Live and precomputed performance"),
        (name = "market", description = "Existing instruments, manual EUR price pins, automatic pricing status, and brokers"),
        (name = "taxation", description = "Austrian capital-gains tax reports"),
        (name = "assets", description = "Custom assets, balances, valuations, and cash accounts"),
        (name = "trading", description = "Browser-owner broker connection, explicit approval, and native Pie configuration. Personal and agent tokens cannot use these REST endpoints."),
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn browser_pie_contract_documents_security_optional_approval_and_tagged_responses() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        for (path, method) in [
            ("/api/trading/pies", "get"), ("/api/trading/pies/{id}", "get"), ("/api/trading/account", "get"),
            ("/api/trading/pie-proposals", "post"), ("/api/trading/proposals/{id}/reconcile", "post"),
            ("/api/trading/instruments", "get"), ("/api/trading/proposals/{id}/approve", "post"),
        ] {
            assert_eq!(document["paths"][path][method]["security"], json!([{"browser_session":[]}]));
        }
        assert_ne!(document["paths"]["/api/trading/proposals/{id}/approve"]["post"]["requestBody"]["required"], true);
        let schema = json!({
            "$ref":"#/components/schemas/Proposal",
            "components":document["components"]
        });
        let validator = jsonschema::validator_for(&schema).unwrap();
        let base = json!({
            "id":"11111111-1111-4111-8111-111111111111","client_request_id":"contract",
            "title":"Review","note":"","environment":"demo","status":"pending","created_at":"2026-11-09T00:00:00Z"
        });
        let mut trades = base.clone();
        trades["kind"] = json!("trades");
        trades["trades"] = json!([]);
        assert!(validator.is_valid(&trades), "{:?}", validator.iter_errors(&trades).collect::<Vec<_>>());
        let mut pie = base;
        pie["kind"] = json!("pie");
        pie["pie"] = json!({
            "mutation":{"action":"delete","pie_id":"20"},"before":null,"expected_settings":null,
            "status":"pending","broker_pie_id":null,"error":null,"submitted_at":null,
            "observation":null,"empty_confirmed_by_owner":false
        });
        assert!(validator.is_valid(&pie), "{:?}", validator.iter_errors(&pie).collect::<Vec<_>>());
        pie["kind"] = json!("trades");
        assert!(!validator.is_valid(&pie));
        trades["kind"] = json!("pie");
        assert!(!validator.is_valid(&trades));
        trades["kind"] = json!("unknown");
        assert!(!validator.is_valid(&trades));
    }
}
