use anyhow::{ensure, Context, Result};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use tabled::Tabled;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::services::events::PortfolioEvent;

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize, ToSchema)]
pub struct FxWac {
    pub units: Decimal,
    pub avg_rate: Decimal,
}

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize, ToSchema)]
pub struct SecWac {
    pub units: Decimal,
    /// Fee-exclusive moving average acquisition cost in EUR (§ 27a Abs. 4 EStG).
    pub average_cost: Decimal,
    pub name: String,
    pub broker: String,
    pub identifier: String,
}

/// JSON tuple encoding avoids collisions between broker and security identifiers.
pub(crate) fn pool_key(broker: &str, identifier: &str) -> String {
    serde_json::to_string(&(broker, identifier)).expect("String tuple serialization cannot fail")
}

pub(crate) fn eur_unit_price(event: &PortfolioEvent) -> Result<Decimal> {
    ensure!(event.price_unit >= dec!(0), "Negative security price");
    if event.currency == "EUR" || event.price_unit == dec!(0) {
        return Ok(event.price_unit);
    }
    // Event rates are native currency units per EUR, not EUR per native unit.
    let rate = event.applied_fx_rate.context("Missing historical security FX rate")?;
    ensure!(rate > dec!(0), "Security FX rate must be positive");
    Ok(event.price_unit / rate)
}

impl SecWac {
    pub(crate) fn update(&mut self, event: &PortfolioEvent) -> Result<()> {
        ensure!(event.units > dec!(0), "Security purchase units must be positive");
        let cost = eur_unit_price(event)?;
        let units = self.units + event.units;
        self.average_cost = (self.units * self.average_cost + event.units * cost) / units;
        self.units = units;
        Ok(())
    }
}
