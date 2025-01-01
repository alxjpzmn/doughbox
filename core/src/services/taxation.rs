use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use tabled::Tabled;
use typeshare::typeshare;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::queries::{composite::get_active_years, fund_report::get_oekb_fund_report_by_id},
    services::market_data::fx_rates::convert_amount,
};

use super::{
    events::{get_events, EventType, PortfolioEvent, TradeDirection},
    files::export_json,
};

#[typeshare]
#[derive(Debug, Serialize, Tabled)]
pub struct AnnualTaxableAmounts {
    #[serde(rename = "Cash Interest")]
    cash_interest: Decimal,
    #[serde(rename = "Share Lending Interest")]
    share_lending_interest: Decimal,
    #[serde(rename = "Capital Gains")]
    capital_gains: Decimal,
    #[serde(rename = "Capital Losses")]
    capital_losses: Decimal,
    #[serde(rename = "Dividends")]
    dividends: Decimal,
    #[serde(rename = "FX Appreciation")]
    fx_appreciation: Decimal,
    #[serde(rename = "Withheld Tax: Dividends")]
    witheld_tax_dividends: Decimal,
    #[serde(rename = "Withheld Tax: Interest")]
    withheld_tax_interest: Decimal,
    #[serde(rename = "Dividend Aequivalents")]
    dividend_aequivalents: Decimal,
}

impl AnnualTaxableAmounts {
    fn round_all(&mut self, dp: u32) {
        self.cash_interest = self.cash_interest.round_dp(dp);
        self.share_lending_interest = self.share_lending_interest.round_dp(dp);
        self.capital_gains = self.capital_gains.round_dp(dp);
        self.dividends = self.dividends.round_dp(dp);
        self.fx_appreciation = self.fx_appreciation.round_dp(dp);
        self.dividend_aequivalents = self.dividend_aequivalents.round_dp(dp);
        self.capital_losses = self.capital_losses.round_dp(dp);
        self.witheld_tax_dividends = self.witheld_tax_dividends.round_dp(dp);
        self.withheld_tax_interest = self.withheld_tax_interest.round_dp(dp);
    }
}

#[typeshare]
#[derive(Debug, Serialize)]
pub struct TaxationReport {
    pub created_at: DateTime<Utc>,
    pub taxable_amounts: BTreeMap<i32, AnnualTaxableAmounts>,
    pub securities_wacs: BTreeMap<String, SecWac>,
    pub currency_wacs: BTreeMap<String, Wac>,
}

#[typeshare]
#[derive(Debug, Tabled, Serialize)]
pub struct Wac {
    pub units: Decimal,
    pub average_cost: Decimal,
}

impl Wac {
    fn round_all(&mut self) {
        self.units = self.units.round_dp(4);
        self.average_cost = self.average_cost.round_dp(2);
    }
}

#[typeshare]
#[derive(Debug, Tabled, Serialize)]
pub struct SecWac {
    pub units: Decimal,
    pub average_cost: Decimal,
    pub weighted_avg_fx_rate: Decimal,
}

impl SecWac {
    fn round_all(&mut self) {
        self.units = self.units.round_dp(4);
        self.average_cost = self.average_cost.round_dp(2);
        self.weighted_avg_fx_rate = self.weighted_avg_fx_rate.round_dp(2);
    }
}

pub struct TaxRates {
    pub interest: Decimal,
    pub capital_gains: Decimal,
    pub dividends: Decimal,
}

pub async fn get_tax_relevant_events(year: i32) -> anyhow::Result<Vec<PortfolioEvent>> {
    let year_start_date_str = format!("{}-01-01", year);
    let year_start_timestamp = DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDate::parse_from_str(&year_start_date_str, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        Utc,
    );

    let year_end_date_str = format!("{}-01-01", year + 1);
    let year_end_timestamp = DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDate::parse_from_str(&year_end_date_str, "%Y-%m-%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        Utc,
    );

    get_events(year_start_timestamp, year_end_timestamp).await
}

pub async fn get_capital_gains_tax_report() -> anyhow::Result<TaxationReport> {
    // Austrian tax rates as of 12/2024
    let tax_rates = TaxRates {
        interest: dec!(0.25),
        capital_gains: dec!(0.275),
        dividends: dec!(0.275),
    };

    // get all years from the first trades until now
    let tax_relevant_years = get_active_years().await?;

    // create hashmap with tax categories for each year
    let mut taxable_amounts = BTreeMap::new();

    // set up Hashmap for keeping track of WAC for different currencies
    let mut currency_wacs = BTreeMap::new();
    // set up Hashmap for keeping track of WAC for different isins
    let mut securities_wacs = BTreeMap::new();

    for year in tax_relevant_years {
        // set up taxable amount for the respective year
        taxable_amounts.insert(
            year,
            AnnualTaxableAmounts {
                cash_interest: dec!(0.0),
                share_lending_interest: dec!(0.0),
                capital_gains: dec!(0.0),
                dividends: dec!(0.0),
                fx_appreciation: dec!(0.0),
                dividend_aequivalents: dec!(0.0),
                capital_losses: dec!(0.0),
                witheld_tax_dividends: dec!(0.0),
                withheld_tax_interest: dec!(0.0),
            },
        );
        // get all tax relevant events for the current year, ordered by date
        let tax_relevant_events = get_tax_relevant_events(year).await?;
        for event in tax_relevant_events {
            match event.event_type {
                EventType::CashInterest => {
                    // ======== CASH INTEREST =========
                    // IF not in EUR, add units to fx pool (basically, you've gotten them for exactly the cost of the
                    // current WAC of said currency, they neither reduce nor increase the currency WAC)
                    // apply the difference of withholding tax in EUR and 25% on those
                    if event.currency != *"EUR" {
                        currency_wacs
                            .entry(event.currency.clone())
                            .and_modify(|entry: &mut Wac| {
                                entry.average_cost = (entry.units * entry.average_cost
                                    + event.applied_fx_rate.unwrap() * event.units)
                                    / (event.units + entry.units);
                            })
                            .and_modify(|entry: &mut Wac| entry.units += event.units)
                            .or_insert(Wac {
                                units: event.units,
                                average_cost: event.applied_fx_rate.unwrap(),
                            });
                    };

                    let taxable_remainder = event.units * event.price_unit;

                    let taxed_amount_eur = if event.currency == "EUR" {
                        taxable_remainder
                    } else {
                        taxable_remainder / event.applied_fx_rate.unwrap()
                    };

                    let withheld_tax =
                        event.withholding_tax_percent.unwrap() * event.price_unit * event.units;

                    let mut withheld_tax_eur = if event.currency == "EUR" {
                        withheld_tax
                    } else {
                        withheld_tax * event.applied_fx_rate.unwrap()
                    };

                    // e.g. for Belgian Tax (30%, used for cash interest by Wise, one can only offset up to 25% of
                    // Austrian KESt)
                    let tax_rate_left =
                        tax_rates.interest - event.withholding_tax_percent.unwrap_or(dec!(0.0));
                    if tax_rate_left < dec!(0) {
                        withheld_tax_eur = withheld_tax_eur
                            - (taxed_amount_eur + withheld_tax_eur) * (tax_rate_left * dec!(-1))
                    }

                    taxable_amounts
                        .entry(year)
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.cash_interest += taxed_amount_eur + withheld_tax_eur
                        })
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.withheld_tax_interest += withheld_tax_eur
                        });
                }
                EventType::ShareInterest => {
                    // ======== SHARE LENDING INTEREST =========
                    // for each year, get all interest rate entries (type needs to be shares) for the year, sorted
                    // by date
                    // apply the difference of withholding tax in EUR and 27.5% on those
                    // IF not in EUR, add to fx pool as the currency units were "acquired" at the
                    // current WAC of said currency, they neither reduce nor increase the currency WAC)

                    if event.currency != *"EUR" {
                        currency_wacs
                            .entry(event.currency.clone())
                            .and_modify(|entry: &mut Wac| {
                                entry.average_cost = (entry.units * entry.average_cost
                                    + event.applied_fx_rate.unwrap() * event.units)
                                    / (event.units + entry.units);
                            })
                            .and_modify(|entry: &mut Wac| entry.units += event.units)
                            .or_insert(Wac {
                                units: event.units,
                                average_cost: event.applied_fx_rate.unwrap(),
                            });
                    };

                    let taxable_remainder = event.units * event.price_unit;

                    let taxed_amount_eur = if event.currency == "EUR" {
                        taxable_remainder
                    } else {
                        taxable_remainder / event.applied_fx_rate.unwrap()
                    };

                    let withheld_tax =
                        event.withholding_tax_percent.unwrap() * event.price_unit * event.units;

                    let mut withheld_tax_eur = if event.currency == "EUR" {
                        withheld_tax
                    } else {
                        withheld_tax * event.applied_fx_rate.unwrap()
                    };

                    // e.g. for Belgian Tax (30%, used for cash interest by Wise, one can only offset up to 25% of
                    // Austrian KESt)
                    let tax_rate_left = tax_rates.capital_gains
                        - event.withholding_tax_percent.unwrap_or(dec!(0.0));
                    if tax_rate_left < dec!(0) {
                        withheld_tax_eur = withheld_tax_eur
                            - (taxed_amount_eur + withheld_tax_eur) * (tax_rate_left * dec!(-1))
                    }

                    taxable_amounts
                        .entry(year)
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.share_lending_interest += taxed_amount_eur + withheld_tax_eur
                        })
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.witheld_tax_dividends += withheld_tax_eur
                        });
                }
                EventType::Dividend => {
                    // ======== DIVIDENDS =========
                    // for each year, get all dividend entries (type needs to be cash) for the year, sorted by date
                    // apply the difference of withholding tax in EUR and 27.5% on those
                    // IF not in EUR, add to fx pool (basically, they were "acquired" for exactly the cost of the
                    // current WAC of said currency, they neither reduce nor increase the currency WAC)
                    if event.currency != *"EUR" {
                        currency_wacs
                            .entry(event.currency.clone())
                            .and_modify(|entry: &mut Wac| {
                                entry.average_cost = (entry.units * entry.average_cost
                                    + event.applied_fx_rate.unwrap() * event.units)
                                    / (event.units + entry.units);
                            })
                            .and_modify(|entry: &mut Wac| entry.units += event.units)
                            .or_insert(Wac {
                                units: event.units,
                                average_cost: event.applied_fx_rate.unwrap(),
                            });
                    };

                    let taxable_remainder = event.units * event.price_unit;

                    let taxed_amount_eur = if event.currency == "EUR" {
                        taxable_remainder
                    } else {
                        taxable_remainder / event.applied_fx_rate.unwrap()
                    };

                    let withheld_tax =
                        event.withholding_tax_percent.unwrap() * event.price_unit * event.units;

                    let mut withheld_tax_eur = if event.currency == "EUR" {
                        withheld_tax
                    } else {
                        withheld_tax * event.applied_fx_rate.unwrap()
                    };

                    // e.g. for Belgian Tax (30%, used for cash interest by Wise, you can only offset up to 25% of
                    // Austrian KESt)
                    let tax_rate_left =
                        tax_rates.dividends - event.withholding_tax_percent.unwrap_or(dec!(0.0));
                    if tax_rate_left < dec!(0) {
                        withheld_tax_eur = withheld_tax_eur
                            - (taxed_amount_eur + withheld_tax_eur) * (tax_rate_left * dec!(-1))
                    }

                    taxable_amounts
                        .entry(year)
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.dividends += taxed_amount_eur + withheld_tax_eur
                        })
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.witheld_tax_dividends += withheld_tax_eur
                        });
                }
                EventType::Trade => {
                    // ======== TRADES =========
                    // for each isin:
                    // create or update entry in WAC hashmap (isin: (units: xx.xx, wac: xx.xx))
                    // IF BUY:
                    //  1. adjust WAC in hashmap: units_in_hashmap * wac + new_trade_units * new_trade_price_per_unit /
                    // (units_in_hashmap + new_trade_units)
                    //  2. adjust total units in hashmap
                    //  3. check whether trade is in fx. if true, get wac for currency and take delta of current
                    //     price vs. wac and add to currency trade list, reduce count of USD @ x.xx

                    match event.clone().direction.unwrap() {
                        TradeDirection::Buy => {
                            securities_wacs
                                .entry(event.clone().identifier.unwrap())
                                .and_modify(|entry: &mut SecWac| {
                                    entry.weighted_avg_fx_rate = (entry.weighted_avg_fx_rate
                                        * entry.units
                                        * entry.average_cost
                                        + event.units
                                            * event.price_unit
                                            * event.applied_fx_rate.unwrap())
                                        / (entry.units * entry.average_cost
                                            + event.units * event.price_unit)
                                })
                                .and_modify(|entry: &mut SecWac| {
                                    entry.average_cost = (entry.average_cost * entry.units
                                        + event.units * event.price_unit)
                                        / (entry.units + event.units)
                                })
                                .and_modify(|entry: &mut SecWac| entry.units += event.units)
                                .or_insert(SecWac {
                                    units: event.units,
                                    average_cost: event.price_unit,
                                    weighted_avg_fx_rate: event.applied_fx_rate.unwrap(),
                                });

                            if event.currency != "EUR" {
                                // count as sell from the outgoing currency
                                let trade_currency = event.currency.clone();

                                let fx_wac =
                                    currency_wacs.entry(trade_currency.clone()).or_insert(Wac {
                                        units: dec!(0.0),
                                        average_cost: dec!(0.0),
                                    });

                                if fx_wac.units > dec!(0.0) {
                                    let eur_exchange_rate = convert_amount(
                                        dec!(1.0),
                                        &event.date.date_naive(),
                                        "EUR",
                                        &trade_currency,
                                    )
                                    .await?;

                                    let fx_delta = fx_wac.average_cost - eur_exchange_rate;

                                    let taxed_amount_eur = ((fx_delta / eur_exchange_rate)
                                        * event.units
                                        * event.price_unit)
                                        / eur_exchange_rate;

                                    taxable_amounts.entry(year).and_modify(
                                        |year: &mut AnnualTaxableAmounts| {
                                            year.fx_appreciation += taxed_amount_eur
                                        },
                                    );

                                    currency_wacs
                                        .entry(trade_currency)
                                        .and_modify(|entry: &mut Wac| entry.units -= event.units)
                                        .and_modify(|entry: &mut Wac| {
                                            if entry.units < dec!(0) {
                                                entry.units = dec!(0.0)
                                            }
                                        });
                                }
                            }
                        }
                        TradeDirection::Sell => {
                            // IF SELL:
                            //  1. adjust units in hashmap: units_in_hashmap - new_trade_units
                            securities_wacs
                                .entry(event.identifier.as_ref().unwrap().clone())
                                .and_modify(|entry: &mut SecWac| entry.units -= event.units);

                            //  2. add difference of sell_price and WAC to tax hashmap for the respective year
                            if event.currency == *"EUR" {
                                let taxable_amount = (event.price_unit
                                    - securities_wacs
                                        .get(&event.identifier.clone().unwrap())
                                        .unwrap()
                                        .average_cost)
                                    * event.units;
                                if taxable_amount > dec!(0.0) {
                                    taxable_amounts.entry(year).and_modify(
                                        |year: &mut AnnualTaxableAmounts| {
                                            year.capital_gains += taxable_amount
                                        },
                                    );
                                } else {
                                    taxable_amounts.entry(year).and_modify(
                                        |year: &mut AnnualTaxableAmounts| {
                                            year.capital_losses -= taxable_amount
                                        },
                                    );
                                }
                            } else {
                                //  3. IF in FX:
                                //      - get total gain of trade in USD
                                //      - get current_fx_rate for pair
                                //      - get wac_fx_rate
                                //      - calculate share wac usd * exchange rate wac * pcs sold => original_cost_eur
                                //      - calculate share_wac usd * pcs sold => original_cost_usd
                                //      - calculate sell_price * pcs sold => sell_amount_usd
                                //      - calculate sell_amount_usd * current_fx_rate => sell_amount_eur
                                //      - calculate (sell_amount_usd - original_cost_usd) * current_fx_rate =>
                                //      capital_gains_eur
                                //      - calculate sell_amount_eur - original_cost_eur => total_taxable_gains_eur
                                //      - total_taxable_gains_eur - capital_gains_eur => fx_appreciation_eur
                                let gain_in_foreign_currency = (event.price_unit
                                    - securities_wacs
                                        .get(&event.identifier.clone().unwrap())
                                        .unwrap()
                                        .average_cost)
                                    * event.units;

                                let eur_exchange_rate = convert_amount(
                                    dec!(1.0),
                                    &event.date.date_naive(),
                                    "EUR",
                                    &event.currency,
                                )
                                .await?;

                                let gain_in_eur = gain_in_foreign_currency / eur_exchange_rate;

                                let fx_wac =
                                    currency_wacs.entry(event.currency.clone()).or_insert(Wac {
                                        units: dec!(0.0),
                                        average_cost: dec!(0.0),
                                    });

                                let instrument_wac = securities_wacs
                                    .entry(event.identifier.clone().unwrap())
                                    .or_insert(SecWac {
                                        units: dec!(0.0),
                                        average_cost: dec!(0.0),
                                        weighted_avg_fx_rate: dec!(0.0),
                                    });

                                let fx_rate_for_buy =
                                    if fx_wac.units > event.units * event.price_unit {
                                        fx_wac.average_cost
                                    } else {
                                        // here, take the weighted average exchange rate during buy
                                        instrument_wac.weighted_avg_fx_rate
                                    };

                                let original_eur_cost =
                                    (instrument_wac.average_cost / fx_rate_for_buy) * event.units;

                                let eur_sell = (event.price_unit / eur_exchange_rate) * event.units;

                                let total_taxable = eur_sell - original_eur_cost;

                                let fx_portion = total_taxable - gain_in_eur;

                                if gain_in_eur > dec!(0.0) {
                                    taxable_amounts.entry(year).and_modify(
                                        |year: &mut AnnualTaxableAmounts| {
                                            year.capital_gains += gain_in_eur
                                        },
                                    );
                                } else {
                                    taxable_amounts.entry(year).and_modify(
                                        |year: &mut AnnualTaxableAmounts| {
                                            year.capital_losses -= gain_in_eur
                                        },
                                    );
                                }

                                taxable_amounts.entry(year).and_modify(
                                    |year: &mut AnnualTaxableAmounts| {
                                        year.fx_appreciation += fx_portion
                                    },
                                );
                            }
                        }
                    }
                }
                EventType::FxConversion => {
                    // ======== FX CONVERSIONS ==
                    // for each exchange, update wac and unit count:
                    // on exchange FROM eur, calculate wac + increase unit count
                    // on exchange TO eur, calculate currency appreciation + decrease unit count
                    let fx_conversion_direction = event.clone().direction.unwrap();
                    match fx_conversion_direction {
                        TradeDirection::Buy => {
                            // revolut stores some top ups as conversions, resulting in EUREUR
                            // identifiers
                            if event.identifier.clone().unwrap() == *"EUREUR" {
                                continue;
                            }
                            currency_wacs
                                .entry(
                                    event.identifier.clone().unwrap()
                                        [event.identifier.unwrap().len() - 3..]
                                        .to_string(),
                                )
                                .and_modify(|entry: &mut Wac| {
                                    entry.average_cost = (entry.units * entry.average_cost
                                        + event.applied_fx_rate.unwrap() * event.units)
                                        / (event.units + entry.units);
                                })
                                .and_modify(|entry: &mut Wac| {
                                    entry.units += event.units * event.applied_fx_rate.unwrap()
                                })
                                .or_insert(Wac {
                                    units: event.units * event.applied_fx_rate.unwrap(),
                                    average_cost: event.applied_fx_rate.unwrap(),
                                });
                        }
                        TradeDirection::Sell => {
                            let applied_fx_rate_reversed =
                                dec!(1.0) / event.applied_fx_rate.unwrap();

                            let destination_currency = event.identifier.clone().unwrap()
                                [event.identifier.clone().unwrap().len() - 3..]
                                .to_string();
                            let origin_currency =
                                event.identifier.clone().unwrap()[..3].to_string();

                            let fx_delta = currency_wacs
                                .get(origin_currency.as_str())
                                .unwrap()
                                .average_cost
                                - applied_fx_rate_reversed;

                            if destination_currency == *"EUR" {
                                let taxed_amount_eur = ((fx_delta / applied_fx_rate_reversed)
                                    * event.units)
                                    / applied_fx_rate_reversed;

                                taxable_amounts.entry(year).and_modify(
                                    |year: &mut AnnualTaxableAmounts| {
                                        year.fx_appreciation += taxed_amount_eur
                                    },
                                );

                                currency_wacs
                                    .entry(origin_currency)
                                    .and_modify(|entry: &mut Wac| entry.units -= event.units)
                                    .and_modify(|entry: &mut Wac| {
                                        if entry.units < dec!(0) {
                                            entry.units = dec!(0.0)
                                        }
                                    });
                            } else {
                                // conversion is e.g. GPBUSD or USDGBP, so doesn't involve EUR
                                // in this case, this counts as sell of the outgoing currency, so
                                // GBP in the case of GBPUSD, which is added to taxable amounts and
                                // reduces unit count
                                // also, it counts as buy of the incoming currency, in the case of
                                // GBPUSD, USD.

                                // count as sell from the outgoing currency
                                let destination_currency = event.identifier.clone().unwrap()
                                    [event.identifier.clone().unwrap().len() - 3..]
                                    .to_string();
                                let origin_currency =
                                    event.identifier.clone().unwrap()[..3].to_string();
                                let eur_exchange_rate = convert_amount(
                                    dec!(1.0),
                                    &event.date.date_naive(),
                                    "EUR",
                                    &origin_currency,
                                )
                                .await
                                .unwrap();

                                let fx_delta = currency_wacs
                                    .get(origin_currency.as_str())
                                    .unwrap()
                                    .average_cost
                                    - eur_exchange_rate;
                                let taxed_amount_eur = ((fx_delta / eur_exchange_rate)
                                    * event.units)
                                    / eur_exchange_rate;

                                taxable_amounts.entry(year).and_modify(
                                    |year: &mut AnnualTaxableAmounts| {
                                        year.fx_appreciation += taxed_amount_eur
                                    },
                                );

                                currency_wacs
                                    .entry(origin_currency)
                                    .and_modify(|entry: &mut Wac| entry.units -= event.units);

                                // take the exchange rate that's applied to the outgoing currency
                                let eur_to_destination_exchange_rate = convert_amount(
                                    dec!(1.0),
                                    &event.date.date_naive(),
                                    "EUR",
                                    &destination_currency,
                                )
                                .await?;

                                // against eur as the price_unit for the new currency
                                // add to units in destination currency
                                currency_wacs
                                    .entry(
                                        event.identifier.clone().unwrap()
                                            [event.identifier.unwrap().len() - 3..]
                                            .to_string(),
                                    )
                                    .and_modify(|entry: &mut Wac| {
                                        entry.average_cost = (entry.average_cost * entry.units
                                            + event.units * eur_to_destination_exchange_rate)
                                            / (entry.units
                                                + event.units * eur_to_destination_exchange_rate)
                                    })
                                    .and_modify(|entry: &mut Wac| {
                                        entry.units += event.units * event.applied_fx_rate.unwrap()
                                    })
                                    .or_insert(Wac {
                                        units: event.units * event.applied_fx_rate.unwrap(),
                                        average_cost: eur_to_destination_exchange_rate,
                                    });
                            };
                        }
                    }
                } // ======== DIVIDEND AEQUIVALENTS ==
                EventType::DividendAequivalent => {
                    let full_report = get_oekb_fund_report_by_id(
                        event.identifier.unwrap().parse::<i32>().unwrap(),
                    )
                    .await?;

                    let report_date = &full_report.date.date_naive();
                    let wacs = securities_wacs
                        .entry(full_report.isin.clone())
                        .or_insert(SecWac {
                            average_cost: dec!(0),
                            units: dec!(0),
                            weighted_avg_fx_rate: dec!(1),
                        });

                    let taxed_amount = (full_report.dividend_aequivalent
                        + full_report.intermittent_dividends)
                        * wacs.units;

                    let taxed_amount_eur =
                        convert_amount(taxed_amount, report_date, &full_report.currency, "EUR")
                            .await?;

                    let withheld_tax = full_report.withheld_dividend * wacs.units;

                    let withheld_tax_eur =
                        convert_amount(withheld_tax, report_date, &full_report.currency, "EUR")
                            .await?;

                    taxable_amounts
                        .entry(year)
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.dividend_aequivalents += taxed_amount_eur
                        })
                        .and_modify(|year: &mut AnnualTaxableAmounts| {
                            year.witheld_tax_dividends += withheld_tax_eur
                        });

                    let cost_adjustment_eur = convert_amount(
                        full_report.wac_adjustment,
                        report_date,
                        &full_report.currency,
                        "EUR",
                    )
                    .await?;

                    // todo verify currency here too (might be that user is holding that isin in USD)
                    securities_wacs
                        .entry(full_report.isin)
                        .and_modify(|entry: &mut SecWac| entry.average_cost += cost_adjustment_eur);
                }
            }
        }
    }
    for (_, amounts) in taxable_amounts.iter_mut() {
        amounts.round_all(2);
    }

    // to only have active positions in the taxation report
    currency_wacs.retain(|_, wac| wac.units != dec!(0));
    securities_wacs.retain(|_, sec_wac| sec_wac.units != dec!(0));

    for (_, wac) in currency_wacs.iter_mut() {
        wac.round_all();
    }

    for (_, sec_wac) in securities_wacs.iter_mut() {
        sec_wac.round_all();
    }

    let taxation_report = TaxationReport {
        created_at: Utc::now(),
        taxable_amounts,
        securities_wacs,
        currency_wacs,
    };

    export_json(&taxation_report, "taxation")?;

    Ok(taxation_report)
}
