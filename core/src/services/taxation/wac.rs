use anyhow::{Context, Result};
use log::{debug, trace};
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

impl FxWac {
    pub(crate) fn round_all(&mut self) {
        trace!(target: "tax_report", "Rounding FX WAC values");

        self.units = self.units.round_dp(4);
        self.avg_rate = self.avg_rate.round_dp(2);
    }

    pub(crate) fn update(&mut self, new_units: Decimal, new_rate: Decimal) {
        debug!(target: "tax_report", "Updating FX WAC with {} units at rate {}", new_units, new_rate);

        let total_units = self.units + new_units;
        self.avg_rate = (self.units * self.avg_rate + new_units * new_rate) / total_units;
        self.units = total_units;
    }
}

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize, ToSchema)]
pub struct SecWac {
    pub units: Decimal,
    pub average_cost: Decimal,
    pub weighted_avg_fx_rate: Decimal,
    pub name: String,
}

impl SecWac {
    pub(crate) fn round_all(&mut self) {
        trace!(target: "tax_report", "Rounding SecWAC values");

        self.units = self.units.round_dp(4);
        self.average_cost = self.average_cost.round_dp(2);
        self.weighted_avg_fx_rate = self.weighted_avg_fx_rate.round_dp(2);
    }

    pub(crate) fn update(&mut self, event: &PortfolioEvent) -> Result<()> {
        debug!(target: "tax_report", "Updating security WAC for event: {:?}", event);

        let new_units = event.units;
        let new_cost = event.price_unit;
        let fx_rate = event
            .applied_fx_rate
            .context("Missing FX rate for security trade")?;

        let total_cost = self.units * self.average_cost + new_units * new_cost;

        if total_cost != dec!(0) {
            self.weighted_avg_fx_rate =
                (self.weighted_avg_fx_rate * self.units * self.average_cost
                    + new_units * new_cost * fx_rate)
                    / total_cost;
        } else {
            self.weighted_avg_fx_rate = dec!(0);
        }

        self.average_cost = total_cost / (self.units + new_units);
        self.units += new_units;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::services::events::{EventType, TradeDirection};

    fn buy(units: Decimal, price: Decimal, fx: Decimal) -> PortfolioEvent {
        PortfolioEvent {
            date: chrono::Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
            event_type: EventType::Trade,
            currency: "EUR".to_string(),
            units,
            price_unit: price,
            identifier: Some("US0378331005".to_string()),
            name: Some("Apple".to_string()),
            direction: Some(TradeDirection::Buy),
            applied_fx_rate: Some(fx),
            withholding_tax_percent: None,
            total: units * price,
            broker: "Trading212".to_string(),
        }
    }

    #[test]
    fn fx_wac_averages_weighted_rate() {
        let mut wac = FxWac {
            units: dec!(100),
            avg_rate: dec!(1.10),
        };
        wac.update(dec!(100), dec!(1.20));
        assert_eq!(wac.units, dec!(200));
        assert_eq!(wac.avg_rate, dec!(1.15));
    }

    #[test]
    fn sec_wac_averages_cost_and_fx() {
        let mut wac = SecWac {
            units: dec!(0),
            average_cost: dec!(0),
            weighted_avg_fx_rate: dec!(0),
            name: "Apple".to_string(),
        };
        wac.update(&buy(dec!(10), dec!(100), dec!(1))).unwrap();
        wac.update(&buy(dec!(10), dec!(200), dec!(1))).unwrap();
        assert_eq!(wac.units, dec!(20));
        assert_eq!(wac.average_cost, dec!(150));
        assert_eq!(wac.weighted_avg_fx_rate, dec!(1));
    }

    #[test]
    fn sec_wac_zero_cost_resets_fx() {
        let mut wac = SecWac {
            units: dec!(0),
            average_cost: dec!(0),
            weighted_avg_fx_rate: dec!(1.2),
            name: "Apple".to_string(),
        };
        wac.update(&buy(dec!(10), dec!(0), dec!(1.1))).unwrap();
        assert_eq!(wac.weighted_avg_fx_rate, dec!(0));
        assert_eq!(wac.units, dec!(10));
        assert_eq!(wac.average_cost, dec!(0));
    }

    #[test]
    fn sec_wac_requires_fx_rate() {
        let mut event = buy(dec!(1), dec!(10), dec!(1));
        event.applied_fx_rate = None;
        let mut wac = SecWac {
            units: dec!(0),
            average_cost: dec!(0),
            weighted_avg_fx_rate: dec!(0),
            name: "Apple".to_string(),
        };
        assert!(wac.update(&event).is_err());
    }
}
