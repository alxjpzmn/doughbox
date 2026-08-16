use rust_decimal::Decimal;
use rust_decimal_macros::dec;

#[derive(Debug, Clone, Copy)]
pub struct TreatyRates {
    pub interest: Decimal,
    pub dividends: Decimal,
    pub capital_gains: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DttIncomeType {
    Interest,
    Dividends,
    CapitalGains,
}

impl TreatyRates {
    pub fn get(&self, income_type: DttIncomeType) -> Decimal {
        match income_type {
            DttIncomeType::Interest => self.interest,
            DttIncomeType::Dividends => self.dividends,
            DttIncomeType::CapitalGains => self.capital_gains,
        }
    }
}

static TREATY_RATES: &[(&str, TreatyRates)] = &[
    (
        "US",
        TreatyRates {
            interest: dec!(0),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
    (
        "IE",
        TreatyRates {
            interest: dec!(0),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
    (
        "BE",
        TreatyRates {
            interest: dec!(0.15),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
    (
        "DE",
        TreatyRates {
            interest: dec!(0),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
    (
        "FR",
        TreatyRates {
            interest: dec!(0),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
    (
        "GB",
        TreatyRates {
            interest: dec!(0),
            dividends: dec!(0.15),
            capital_gains: dec!(0),
        },
    ),
];

pub fn isin_to_country(isin: &str) -> Option<&'static str> {
    let bytes = isin.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1].is_ascii_alphabetic() {
        let prefix = &isin[..2];
        if let Some((country, _)) = TREATY_RATES.iter().find(|(c, _)| *c == prefix) {
            return Some(country);
        }
    }
    None
}

pub fn broker_to_country(broker: &str) -> Option<&'static str> {
    match broker {
        "Wise" => Some("BE"),
        _ => None,
    }
}

pub fn treaty_rate(country: &str, income_type: DttIncomeType) -> Option<Decimal> {
    TREATY_RATES
        .iter()
        .find(|(c, _)| *c == country)
        .map(|(_, rates)| rates.get(income_type))
}

pub fn determine_source_country(isin: Option<&str>, broker: &str) -> Option<&'static str> {
    if let Some(isin) = isin {
        if let Some(country) = isin_to_country(isin) {
            return Some(country);
        }
    }
    broker_to_country(broker)
}
