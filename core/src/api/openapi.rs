use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use super::handlers::{
    auth::{self, LoginRequestData},
    market, performance, portfolio, settings, taxation, timeline, trades,
};
use crate::{
    api::errors::{ErrorDetails, ErrorResponse},
    database::models::{
        dividend::Dividend,
        fx_conversion::FxConversion,
        instrument::{Instrument, InstrumentDetails},
        interest::InterestPayment,
        performance::PerformanceSignal,
        position::PositionWithName,
        position::PositionWithValueAndAllocation,
        trade::Trade,
    },
    services::{
        events::{EventType, PortfolioEvent, TradeDirection},
        market_data::prices::types::{
            PriceFeedStatus, PriceQuote, PriceSchedulerSettings, PricingStatus,
        },
        performance::{BuyIn, PortfolioPerformance, PositionPerformance},
        portfolio::PortfolioOverview,
        shared::settings::ApplicationSettings,
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
        description = "REST API for portfolio allocations, trades, buy-ins, performance, Austrian tax reports, and managing existing instrument names and manual EUR price pins.\n\n\
            Authenticate with a session cookie from `POST /api/login` or `Authorization: Bearer $API_TOKEN`.",
        version = "0.1.0"
    ),
    paths(
        auth::login,
        auth::logout,
        auth::auth_state,
        portfolio::portfolio,
        portfolio::positions,
        timeline::timeline,
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
        market::rename,
        market::set_price,
        market::unpin_price,
        market::brokers,
        market::prices,
        market::save_scheduler,
        market::refresh_prices,
        market::refresh_price_feed,
        market::reset_price_feed,
        settings::get_settings,
        settings::save_settings,
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
        BuyIn,
        Dividend,
        InterestPayment,
        FxConversion,
        PortfolioPerformance,
        PositionPerformance,
        PerformanceSignal,
        Instrument,
        InstrumentDetails,
        market::RenameInstrumentRequest,
        market::ManualPriceRequest,
        PricingStatus,
        PriceFeedStatus,
        PriceSchedulerSettings,
        PriceQuote,
        ApplicationSettings,
        TaxationReport,
        AnnualTaxableAmounts,
        SecWac,
        FxWac,
        TransactionTaxImpact,
    )),
    tags(
        (name = "auth", description = "Session login and logout"),
        (name = "portfolio", description = "Allocations, holdings, and timeline"),
        (name = "trades", description = "Trades, buy-ins, dividends, interest, and FX"),
        (name = "performance", description = "Live and precomputed performance"),
        (name = "market", description = "Existing instruments, manual EUR price pins, automatic pricing status, and brokers"),
        (name = "taxation", description = "Austrian capital-gains tax reports"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;
