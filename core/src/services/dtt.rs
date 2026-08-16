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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isin_prefix_maps_to_treaty_country() {
        assert_eq!(isin_to_country("US0378331005"), Some("US"));
        assert_eq!(isin_to_country("IE00B4L5Y983"), Some("IE"));
        assert_eq!(isin_to_country("BE0974293251"), Some("BE"));
        assert_eq!(isin_to_country("DE0007164600"), Some("DE"));
        assert_eq!(isin_to_country("FR0000120271"), Some("FR"));
        assert_eq!(isin_to_country("GB0002374006"), Some("GB"));
    }

    #[test]
    fn isin_prefix_without_treaty_is_none() {
        assert_eq!(isin_to_country("AT0000743059"), None);
        assert_eq!(isin_to_country("NL0010273215"), None);
        assert_eq!(isin_to_country("1"), None);
        assert_eq!(isin_to_country(""), None);
    }

    #[test]
    fn wise_broker_maps_to_belgium() {
        assert_eq!(broker_to_country("Wise"), Some("BE"));
        assert_eq!(broker_to_country("Trade Republic"), None);
    }

    #[test]
    fn treaty_rates_match_table() {
        assert_eq!(treaty_rate("US", DttIncomeType::Dividends), Some(dec!(0.15)));
        assert_eq!(treaty_rate("US", DttIncomeType::Interest), Some(dec!(0)));
        assert_eq!(treaty_rate("US", DttIncomeType::CapitalGains), Some(dec!(0)));
        assert_eq!(treaty_rate("BE", DttIncomeType::Interest), Some(dec!(0.15)));
        assert_eq!(treaty_rate("AT", DttIncomeType::Dividends), None);
    }

    #[test]
    fn source_country_prefers_isin_then_broker() {
        assert_eq!(
            determine_source_country(Some("US0378331005"), "Wise"),
            Some("US")
        );
        assert_eq!(
            determine_source_country(Some("AT0000743059"), "Wise"),
            Some("BE")
        );
        assert_eq!(determine_source_country(None, "Wise"), Some("BE"));
        assert_eq!(determine_source_country(None, "Revolut"), None);
    }
}
