use std::{
    collections::{BTreeMap, HashSet},
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use chrono::DateTime;
use rust_decimal::Decimal;

use super::{
    DividendCashAction, PieCreateSettings, PieDetail, PieOperationInput, PieResult, PieSettings,
    PieSettingsChanges, PieStatus, PreparedPieMutation,
};
use crate::services::trading::provider::Instrument;

pub fn validate_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().all(|c| c.is_ascii_digit())
        || !value.parse::<i64>().is_ok_and(|id| id > 0)
    {
        bail!("Pie ID must be a positive int64 decimal string");
    }
    Ok(())
}
pub fn fill_missing_targets(detail: &mut PieDetail) -> Result<()> {
    let Some(settings) = detail.settings.as_mut() else { return Ok(()) };
    if settings.instrument_shares.as_ref().is_some_and(|shares| !shares.is_empty()) { return Ok(()) }
    let instruments = detail.instruments.as_ref().ok_or_else(|| anyhow!("Trading 212 omitted instrumentShares and instruments. Refresh the Pie; Doughbox will not invent allocation targets."))?;
    let mut shares = BTreeMap::new();
    let mut total = Decimal::ZERO;
    for instrument in instruments {
        let ticker = instrument.ticker.as_deref().filter(|ticker| !ticker.is_empty()).ok_or_else(|| anyhow!("Trading 212 omitted a ticker for an expectedShare target."))?;
        let expected = instrument.expected_share.as_deref().ok_or_else(|| anyhow!("Trading 212 omitted instrumentShares and an instrument has no expectedShare. Refresh the Pie; Doughbox will not invent allocation targets."))?;
        let weight = decimal(expected)?;
        if weight <= Decimal::ZERO { bail!("Trading 212 expectedShare must be positive when instrumentShares is absent.") }
        total = total.checked_add(weight).ok_or_else(|| anyhow!("expectedShare total overflow"))?;
        shares.insert(ticker.to_string(), Some(expected.to_string()));
    }
    if shares.is_empty() || total != Decimal::ONE {
        bail!("Trading 212 omitted instrumentShares and expectedShare values do not sum to 1. Refresh the Pie; Doughbox will not invent allocation targets.");
    }
    settings.instrument_shares = Some(shares);
    settings.instrument_shares_source = Some("expected_share".into());
    Ok(())
}

#[cfg(test)]
mod fill_tests {
    use super::*;
    use crate::services::trading::pies::{PieDetail, PieInstrument, PieSettings};
    use chrono::Utc;

    #[test]
    fn reconstructs_targets_only_when_expected_shares_sum_to_one() {
        let mut detail = PieDetail {
            fetched_at: Utc::now(), id: "10".into(),
            settings: Some(PieSettings::default()),
            instruments: Some(vec![
                PieInstrument { ticker: Some("AAPL_US_EQ".into()), expected_share: Some("0.5".into()), owned_quantity: Some("3".into()), current_share: None, issues: None, result: None },
                PieInstrument { ticker: Some("MSFT_US_EQ".into()), expected_share: Some("0.5".into()), owned_quantity: None, current_share: None, issues: None, result: None },
            ]),
        };
        fill_missing_targets(&mut detail).unwrap();
        let settings = detail.settings.unwrap();
        assert_eq!(settings.instrument_shares_source.as_deref(), Some("expected_share"));
        assert_eq!(settings.instrument_shares.unwrap()["AAPL_US_EQ"].as_deref(), Some("0.5"));
        detail = PieDetail { fetched_at: Utc::now(), id: "10".into(), settings: Some(PieSettings::default()), instruments: Some(vec![
            PieInstrument { ticker: Some("AAPL_US_EQ".into()), expected_share: Some("0.4".into()), owned_quantity: None, current_share: None, issues: None, result: None },
        ]) };
        assert!(fill_missing_targets(&mut detail).is_err());
        assert!(detail.settings.unwrap().instrument_shares.is_none());
    }
}
fn decimal(value: &str) -> Result<Decimal> {
    Decimal::from_str_exact(value)
        .or_else(|_| Decimal::from_scientific(value))
        .map_err(|_| anyhow!("Pie value must be an exact decimal"))
}
fn name(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("Pie name cannot be blank");
    }
    Ok(())
}
fn goal(value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        if decimal(value)? < Decimal::ZERO {
            bail!("Pie goal must be nonnegative");
        }
        serde_json::Number::from_str(value)
            .map_err(|_| anyhow!("Pie goal must encode as an exact JSON decimal"))?;
    }
    Ok(())
}
fn end_date(value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        DateTime::parse_from_rfc3339(value)
            .map_err(|_| anyhow!("New Pie end date must be RFC3339"))?;
    }
    Ok(())
}
fn allocations(shares: &BTreeMap<String, String>, instruments: &[Instrument]) -> Result<()> {
    if !(1..=50).contains(&shares.len()) {
        bail!("Pie allocation requires 1–50 distinct STOCK/ETF instruments");
    }
    // The connection catalog already filters the broker's STOCK/ETF instrument types.
    let catalog: HashSet<&str> = instruments
        .iter()
        .map(|item| item.ticker.as_str())
        .collect();
    let mut total = Decimal::ZERO;
    for (ticker, raw) in shares {
        if !catalog.contains(ticker.as_str()) {
            bail!("Pie allocation ticker is not in the connected STOCK/ETF catalog: {ticker}");
        }
        let mut parts = raw.split('.');
        let integer = parts.next().unwrap_or_default();
        let fraction = parts.next();
        if integer.is_empty()
            || !integer.bytes().all(|c| c.is_ascii_digit())
            || fraction.is_some_and(|part| {
                part.is_empty() || part.len() > 4 || !part.bytes().all(|c| c.is_ascii_digit())
            })
            || parts.next().is_some()
        {
            bail!(
                "Pie weights must be fractional decimal strings with at most four decimal places"
            );
        }
        let weight = decimal(raw)?;
        if weight <= Decimal::ZERO || weight > Decimal::ONE {
            bail!("Pie weights must be greater than 0 and at most 1");
        }
        serde_json::Number::from_str(raw)
            .map_err(|_| anyhow!("Pie weight must encode as an exact JSON decimal"))?;
        total = total
            .checked_add(weight)
            .ok_or_else(|| anyhow!("Pie allocation total is too large"))?;
    }
    if total != Decimal::ONE {
        bail!("Pie allocation weights must sum exactly to 1");
    }
    Ok(())
}

/// Validate only newly supplied fields. Unchanged broker representations are
/// preserved during preparation, not normalized or revalidated as new intent.
pub fn validate_operation(operation: &PieOperationInput, instruments: &[Instrument]) -> Result<()> {
    match operation {
        PieOperationInput::Create { settings } => {
            name(&settings.name)?;
            allocations(&settings.instrument_shares, instruments)?;
            goal(settings.goal.as_deref())?;
            end_date(settings.end_date.as_deref())?;
        }
        PieOperationInput::Update { pie_id, changes } => {
            validate_id(pie_id)?;
            if changes == &PieSettingsChanges::default() {
                bail!("Pie update must contain an actual change");
            }
            if let Some(value) = &changes.name {
                name(value)?;
            }
            if let Some(shares) = &changes.instrument_shares {
                allocations(shares, instruments)?;
            }
            goal(changes.goal.as_deref())?;
            end_date(changes.end_date.as_deref())?;
        }
        PieOperationInput::Duplicate {
            pie_id,
            name: value,
            ..
        } => {
            validate_id(pie_id)?;
            name(value)?;
        }
        PieOperationInput::Delete { pie_id } => validate_id(pie_id)?,
    }
    Ok(())
}

fn complete(detail: &PieDetail) -> Result<PieCreateSettings> {
    validate_id(&detail.id)?;
    let settings = detail
        .settings
        .as_ref()
        .ok_or_else(|| anyhow!("Pie settings snapshot is missing"))?;
    let name = settings
        .name
        .clone()
        .ok_or_else(|| anyhow!("Pie name snapshot is missing"))?;
    let dividend_cash_action = match settings.dividend_cash_action.as_deref() {
        Some("REINVEST") => DividendCashAction::REINVEST,
        Some("TO_ACCOUNT_CASH") => DividendCashAction::ToAccountCash,
        _ => bail!("Pie dividend action snapshot is missing or unsupported"),
    };
    let instrument_shares = settings
        .instrument_shares
        .as_ref()
        .ok_or_else(|| anyhow!("Pie target allocation snapshot is missing"))?
        .iter()
        .map(|(ticker, weight)| {
            let weight = weight
                .clone()
                .ok_or_else(|| anyhow!("Pie target weight snapshot is missing"))?;
            decimal(&weight)?;
            Ok((ticker.clone(), weight))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if instrument_shares.is_empty() {
        bail!("Pie target allocation snapshot is empty");
    }
    Ok(PieCreateSettings {
        name,
        dividend_cash_action,
        instrument_shares,
        icon: settings.icon.clone(),
        goal: settings.goal.clone(),
        end_date: settings.end_date.clone(),
    })
}
fn public(settings: &PieCreateSettings) -> PieSettings {
    PieSettings {
        name: Some(settings.name.clone()),
        icon: settings.icon.clone(),
        dividend_cash_action: Some(
            match settings.dividend_cash_action {
                DividendCashAction::REINVEST => "REINVEST",
                DividendCashAction::ToAccountCash => "TO_ACCOUNT_CASH",
            }
            .into(),
        ),
        goal: settings.goal.clone(),
        end_date: settings.end_date.clone(),
        instrument_shares: Some(
            settings
                .instrument_shares
                .iter()
                .map(|(ticker, weight)| (ticker.clone(), Some(weight.clone())))
                .collect(),
        ),
        instrument_shares_source: None,
    }
}
fn source<'a>(before: &'a Option<PieDetail>, pie_id: &str) -> Result<&'a PieDetail> {
    validate_id(pie_id)?;
    let detail = before
        .as_ref()
        .ok_or_else(|| anyhow!("Source Pie does not exist"))?;
    if detail.id.parse::<i64>().ok() != pie_id.parse::<i64>().ok() {
        bail!("Source Pie ID disagrees with the requested ID");
    }
    Ok(detail)
}
pub fn keeps_current_name(current: Option<&str>, next: &str) -> bool {
    current == Some(next)
}

/// Trading 212's update endpoint returns HTTP 400
/// "Check the id and rename the pie if you want to apply changes" when the
/// sent name equals the live name. A GET of that id can still succeed.
/// Do not invent a suffix or send a second write.
pub fn unchanged_name_update(pie_id: &str) -> String {
    format!("Trading 212 rejects Pie updates that keep the current name. For Pie #{pie_id}, set the next versioned name: if it ends with \" v\" and a positive integer, increment it (\"Growth v2\" → \"Growth v3\"); otherwise append \" v1\". Doughbox will not invent that version.")
}

/// Preparation is pure and never performs any broker mutation.
pub fn prepare(operation: PieOperationInput, before: Option<PieDetail>) -> Result<PieResult> {
    let (mutation, expected_settings) = match operation {
        PieOperationInput::Create { settings } => {
            if before.is_some() {
                bail!("Pie creation must not have a source snapshot");
            }
            let expected = public(&settings);
            (PreparedPieMutation::Create { settings }, Some(expected))
        }
        PieOperationInput::Update { pie_id, changes } => {
            let detail = source(&before, &pie_id)?;
            let mut settings = complete(detail)?;
            if let Some(value) = changes.name {
                settings.name = value;
            }
            if let Some(value) = changes.dividend_cash_action {
                settings.dividend_cash_action = value;
            }
            if let Some(value) = changes.instrument_shares {
                settings.instrument_shares = value;
            }
            if let Some(value) = changes.icon {
                settings.icon = Some(value);
            }
            if let Some(value) = changes.goal {
                settings.goal = Some(value);
            }
            if let Some(value) = changes.end_date {
                settings.end_date = Some(value);
            }
            let expected = public(&settings);
            if detail
                .settings
                .as_ref()
                .is_some_and(|old| settings_equal(old, &expected))
            {
                bail!("Pie update does not change its settings");
            }
            if keeps_current_name(
                detail.settings.as_ref().and_then(|old| old.name.as_deref()),
                &settings.name,
            ) {
                bail!("{}", unchanged_name_update(&pie_id));
            }
            (
                PreparedPieMutation::Update { pie_id, settings },
                Some(expected),
            )
        }
        PieOperationInput::Duplicate { pie_id, name, icon } => {
            let detail = source(&before, &pie_id)?;
            let source_settings = complete(detail)?;
            // Composition only: no inherited goal, dividend policy, holdings or AutoInvest claim.
            let expected = PieSettings {
                name: Some(name.clone()),
                icon: icon.clone(),
                instrument_shares: Some(
                    source_settings
                        .instrument_shares
                        .into_iter()
                        .map(|(ticker, weight)| (ticker, Some(weight)))
                        .collect(),
                ),
                ..PieSettings::default()
            };
            (
                PreparedPieMutation::Duplicate { pie_id, name, icon },
                Some(expected),
            )
        }
        PieOperationInput::Delete { pie_id } => {
            let detail = source(&before, &pie_id)?;
            complete(detail)?;
            ensure_empty(detail)?;
            (PreparedPieMutation::Delete { pie_id }, None)
        }
    };
    Ok(PieResult {
        mutation,
        account_currency: None,
        before,
        expected_settings,
        status: PieStatus::Pending,
        broker_pie_id: None,
        error: None,
        submitted_at: None,
        observation: None,
        empty_confirmed_by_owner: false,
    })
}

pub fn ensure_empty(detail: &PieDetail) -> Result<()> {
    let instruments = detail.instruments.as_ref().ok_or_else(|| {
        anyhow!("Pie holdings evidence is missing; empty the Pie in Trading 212 before deletion")
    })?;
    for instrument in instruments {
        let quantity = instrument
            .owned_quantity
            .as_deref()
            .ok_or_else(|| anyhow!("Pie owned quantity is unknown; deletion is blocked"))?;
        if decimal(quantity)? != Decimal::ZERO {
            bail!("Pie has investments; empty the Pie in Trading 212 before deletion");
        }
    }
    // Public data does not reliably establish free Pie cash. The owner must
    // separately attest it is empty; broker rejection remains authoritative.
    Ok(())
}
fn numeric_equal(left: &str, right: &str) -> bool {
    matches!((decimal(left), decimal(right)), (Ok(a), Ok(b)) if a == b)
}
fn optional_numeric_equal(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => numeric_equal(a, b),
        _ => false,
    }
}
fn date_equal(left: &str, right: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(left),
        DateTime::parse_from_rfc3339(right),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}
fn optional_date_equal(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => date_equal(a, b),
        _ => false,
    }
}
fn shares_equal(
    left: &Option<BTreeMap<String, Option<String>>>,
    right: &Option<BTreeMap<String, Option<String>>>,
) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    left.len() == right.len() && !left.is_empty() && left.iter().all(|(ticker, weight)| {
        matches!((weight, right.get(ticker)), (Some(a), Some(Some(b))) if numeric_equal(a, b))
    })
}
fn settings_equal(left: &PieSettings, right: &PieSettings) -> bool {
    left.name == right.name
        && left.icon == right.icon
        && left.dividend_cash_action == right.dividend_cash_action
        && optional_numeric_equal(&left.goal, &right.goal)
        && optional_date_equal(&left.end_date, &right.end_date)
        && shares_equal(&left.instrument_shares, &right.instrument_shares)
}

/// Ignore prices/current weights/holdings, but require complete, stable writable
/// settings and target membership. Delete holdings are checked separately.
pub fn configuration_matches(before: &PieDetail, actual: &PieDetail) -> bool {
    if before.id.parse::<i64>().ok() != actual.id.parse::<i64>().ok()
        || complete(before).is_err()
        || complete(actual).is_err()
    {
        return false;
    }
    match (&before.settings, &actual.settings) {
        (Some(before), Some(actual)) => settings_equal(before, actual),
        _ => false,
    }
}

/// Compare only approved evidence; optional unspecified settings are not claims
/// about broker defaults. Updates compare their complete merged snapshot.
pub fn result_matches(result: &PieResult, actual: &PieDetail) -> bool {
    if validate_id(&actual.id).is_err()
        || matches!(result.mutation, PreparedPieMutation::Delete { .. })
    {
        return false;
    }
    if let Some(id) = &result.broker_pie_id {
        if id.parse::<i64>().ok() != actual.id.parse::<i64>().ok() {
            return false;
        }
    }
    if let PreparedPieMutation::Update { pie_id, .. } = &result.mutation {
        if pie_id.parse::<i64>().ok() != actual.id.parse::<i64>().ok() {
            return false;
        }
    }
    let (Some(expected), Some(actual)) = (&result.expected_settings, &actual.settings) else {
        return false;
    };
    if matches!(result.mutation, PreparedPieMutation::Update { .. }) {
        return expected.name.is_some()
            && expected.dividend_cash_action.is_some()
            && settings_equal(expected, actual);
    }
    expected.name.is_some()
        && expected.name == actual.name
        && expected
            .icon
            .as_ref()
            .is_none_or(|value| actual.icon.as_ref() == Some(value))
        && expected
            .dividend_cash_action
            .as_ref()
            .is_none_or(|value| actual.dividend_cash_action.as_ref() == Some(value))
        && expected.goal.as_ref().is_none_or(|value| {
            actual
                .goal
                .as_ref()
                .is_some_and(|actual| numeric_equal(value, actual))
        })
        && expected.end_date.as_ref().is_none_or(|value| {
            actual
                .end_date
                .as_ref()
                .is_some_and(|actual| date_equal(value, actual))
        })
        && shares_equal(&expected.instrument_shares, &actual.instrument_shares)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn catalog() -> Vec<Instrument> {
        ["AAPL_US_EQ", "MSFT_US_EQ"]
            .into_iter()
            .map(|ticker| Instrument {
                ticker: ticker.into(),
                name: ticker.into(),
                currency: "USD".into(),
                isin: "test".into(),
            })
            .collect()
    }
    fn shares(a: &str, b: &str) -> BTreeMap<String, String> {
        [
            ("AAPL_US_EQ".into(), a.into()),
            ("MSFT_US_EQ".into(), b.into()),
        ]
        .into()
    }
    fn settings() -> PieCreateSettings {
        PieCreateSettings {
            name: "  Original broker name  ".into(),
            dividend_cash_action: DividendCashAction::REINVEST,
            instrument_shares: shares("0.60", "0.40"),
            icon: Some("Original icon".into()),
            goal: Some("1000.00".into()),
            end_date: Some("legacy broker date".into()),
        }
    }
    fn snapshot() -> PieDetail {
        PieDetail {
            fetched_at: Utc::now(),
            id: "10".into(),
            settings: Some(public(&settings())),
            instruments: Some(vec![]),
        }
    }
    fn update(changes: PieSettingsChanges) -> PieOperationInput {
        PieOperationInput::Update {
            pie_id: "10".into(),
            changes,
        }
    }

    #[test]
    fn exact_boundary_weights_are_valid_without_normalization() {
        let mut settings = settings();
        settings.end_date = None;
        settings.instrument_shares = shares("0.0001", "0.9999");
        let operation = PieOperationInput::Create { settings };
        validate_operation(&operation, &catalog()).unwrap();
        let prepared = prepare(operation, None).unwrap();
        let PreparedPieMutation::Create { settings } = prepared.mutation else {
            panic!()
        };
        assert_eq!(settings.instrument_shares["AAPL_US_EQ"], "0.0001");
        assert_eq!(settings.instrument_shares["MSFT_US_EQ"], "0.9999");
    }

    #[test]
    fn invalid_allocation_totals_precision_ranges_and_catalog_are_rejected() {
        for values in [
            shares("0.60", "0.39"),
            shares("0.00010", "0.9999"),
            shares("0", "1"),
            shares("1.1", "0.1"),
            shares("1e-1", "0.9"),
            shares("00.6", "0.4"),
        ] {
            assert!(validate_operation(
                &update(PieSettingsChanges {
                    instrument_shares: Some(values),
                    ..Default::default()
                }),
                &catalog()
            )
            .is_err());
        }
        let mut unknown = shares("0.6", "0.4");
        unknown.insert("NOT_A_STOCK".into(), "0.1".into());
        assert!(allocations(&unknown, &catalog()).is_err());
        assert!(allocations(&BTreeMap::new(), &catalog()).is_err());
        let fifty_one = (0..51).map(|i| (format!("T{i}"), "0.01".into())).collect();
        assert!(allocations(&fifty_one, &[]).is_err());
    }

    #[test]
    fn new_fields_are_validated_but_unchanged_legacy_settings_are_preserved() {
        let input = update(PieSettingsChanges {
            name: Some("Renamed Pie".into()),
            instrument_shares: Some(shares("0.7", "0.3")),
            ..Default::default()
        });
        validate_operation(&input, &catalog()).unwrap();
        let result = prepare(input, Some(snapshot())).unwrap();
        let PreparedPieMutation::Update {
            settings: merged, ..
        } = result.mutation
        else {
            panic!()
        };
        let original = settings();
        assert_eq!(merged.name, "Renamed Pie");
        assert_eq!(merged.end_date, original.end_date);
        assert_eq!(merged.goal, original.goal);
        assert_eq!(merged.icon, original.icon);
        for changes in [
            PieSettingsChanges {
                instrument_shares: Some(shares("0.7", "0.3")),
                ..Default::default()
            },
            PieSettingsChanges {
                name: Some(original.name.clone()),
                instrument_shares: Some(shares("0.7", "0.3")),
                ..Default::default()
            },
        ] {
            let error = prepare(update(changes), Some(snapshot())).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("keep the current name"), "{message}");
            assert!(message.contains("Pie #10"), "{message}");
            assert!(message.contains("append \" v1\""), "{message}");
            assert!(message.contains("will not invent that version"), "{message}");
        }
        let spaced = prepare(
            update(PieSettingsChanges {
                name: Some(format!("{} ", original.name)),
                ..Default::default()
            }),
            Some(snapshot()),
        )
        .unwrap();
        let PreparedPieMutation::Update { settings, .. } = spaced.mutation else {
            panic!()
        };
        assert_eq!(settings.name, format!("{} ", original.name));
        assert!(validate_operation(
            &update(PieSettingsChanges {
                end_date: Some("legacy broker date".into()),
                ..Default::default()
            }),
            &catalog()
        )
        .is_err());
        assert!(validate_operation(
            &update(PieSettingsChanges {
                goal: Some("-1".into()),
                ..Default::default()
            }),
            &catalog()
        )
        .is_err());
        assert!(validate_operation(
            &update(PieSettingsChanges {
                name: Some(" \t".into()),
                ..Default::default()
            }),
            &catalog()
        )
        .is_err());
        assert!(validate_operation(
            &update(PieSettingsChanges {
                name: Some("A name far longer than twenty characters is valid".into()),
                ..Default::default()
            }),
            &catalog()
        )
        .is_ok());
    }

    #[test]
    fn numeric_and_instant_equivalent_updates_are_no_ops() {
        let before = snapshot();
        assert!(prepare(
            update(PieSettingsChanges {
                instrument_shares: Some(shares("0.6", "0.4")),
                ..Default::default()
            }),
            Some(before.clone())
        )
        .is_err());
        assert!(prepare(
            update(PieSettingsChanges {
                goal: Some("1000".into()),
                ..Default::default()
            }),
            Some(before.clone())
        )
        .is_err());
        assert!(prepare(update(PieSettingsChanges::default()), Some(before.clone())).is_err());
        let mut dated = before;
        dated.settings.as_mut().unwrap().end_date = Some("2030-01-01T12:00:00Z".into());
        assert!(prepare(
            update(PieSettingsChanges {
                end_date: Some("2030-01-01T13:00:00+01:00".into()),
                ..Default::default()
            }),
            Some(dated)
        )
        .is_err());
    }

    #[test]
    fn incomplete_settings_and_route_id_disagreement_cannot_be_prepared() {
        let mut before = snapshot();
        before
            .settings
            .as_mut()
            .unwrap()
            .instrument_shares
            .as_mut()
            .unwrap()
            .insert("AAPL_US_EQ".into(), None);
        assert!(prepare(
            update(PieSettingsChanges {
                name: Some("New".into()),
                ..Default::default()
            }),
            Some(before)
        )
        .is_err());
        let mut before = snapshot();
        before.settings.as_mut().unwrap().dividend_cash_action = None;
        assert!(prepare(
            PieOperationInput::Duplicate {
                pie_id: "10".into(),
                name: "Copy".into(),
                icon: None
            },
            Some(before)
        )
        .is_err());
        assert!(prepare(
            PieOperationInput::Delete {
                pie_id: "11".into()
            },
            Some(snapshot())
        )
        .is_err());
        for value in ["0", "-1", "1.0", "9223372036854775808", "", "1/x", " 1"] {
            assert!(validate_id(value).is_err());
        }
        validate_id("9223372036854775807").unwrap();
    }

    #[test]
    fn configuration_comparison_ignores_market_data_but_detects_target_drift() {
        let before = snapshot();
        let mut actual = before.clone();
        actual.instruments = None;
        actual.settings.as_mut().unwrap().instrument_shares = Some(
            shares("0.6", "0.4")
                .into_iter()
                .map(|(k, v)| (k, Some(v)))
                .collect(),
        );
        assert!(configuration_matches(&before, &actual));
        actual
            .settings
            .as_mut()
            .unwrap()
            .instrument_shares
            .as_mut()
            .unwrap()
            .insert("AAPL_US_EQ".into(), Some("0.7".into()));
        assert!(!configuration_matches(&before, &actual));
        actual = before.clone();
        actual.settings.as_mut().unwrap().name = None;
        assert!(!configuration_matches(&before, &actual));
        actual = before.clone();
        actual.settings.as_mut().unwrap().end_date = Some("a different legacy date".into());
        assert!(!configuration_matches(&before, &actual));
    }

    #[test]
    fn missing_owned_quantity_is_not_zero_and_no_cash_balance_is_inferred() {
        let mut before = snapshot();
        ensure_empty(&before).unwrap();
        before.instruments = None;
        assert!(ensure_empty(&before).is_err());
        let instrument = super::super::PieInstrument {
            ticker: Some("AAPL_US_EQ".into()),
            owned_quantity: None,
            current_share: Some("0".into()),
            expected_share: Some("0".into()),
            issues: None,
            result: None,
        };
        before.instruments = Some(vec![instrument]);
        assert!(ensure_empty(&before).is_err());
        before.instruments.as_mut().unwrap()[0].owned_quantity = Some("0.0000".into());
        ensure_empty(&before).unwrap();
        let result = prepare(
            PieOperationInput::Delete {
                pie_id: "10".into(),
            },
            Some(before.clone()),
        )
        .unwrap();
        assert!(!result.empty_confirmed_by_owner);
        before.instruments.as_mut().unwrap()[0].owned_quantity = Some("0.0001".into());
        assert!(ensure_empty(&before).is_err());
    }

    #[test]
    fn duplicate_expected_evidence_contains_only_composition_name_and_requested_icon() {
        let result = prepare(
            PieOperationInput::Duplicate {
                pie_id: "10".into(),
                name: "Copy".into(),
                icon: None,
            },
            Some(snapshot()),
        )
        .unwrap();
        let expected = result.expected_settings.as_ref().unwrap();
        assert_eq!(expected.dividend_cash_action, None);
        assert_eq!(expected.goal, None);
        assert_eq!(expected.end_date, None);
        let mut copy = snapshot();
        copy.id = "11".into();
        copy.settings.as_mut().unwrap().name = Some("Copy".into());
        copy.settings.as_mut().unwrap().dividend_cash_action = Some("TO_ACCOUNT_CASH".into());
        copy.settings.as_mut().unwrap().goal = None;
        copy.settings.as_mut().unwrap().end_date = None;
        assert!(result_matches(&result, &copy));
        copy.settings.as_mut().unwrap().instrument_shares = None;
        assert!(!result_matches(&result, &copy));
    }

    #[test]
    fn readback_requires_all_approved_fields_and_update_preserves_optional_absence() {
        let before = snapshot();
        let mut result = prepare(
            update(PieSettingsChanges {
                name: Some("Changed".into()),
                ..Default::default()
            }),
            Some(before.clone()),
        )
        .unwrap();
        result.broker_pie_id = Some("10".into());
        let mut actual = before;
        actual.settings.as_mut().unwrap().name = Some("Changed".into());
        assert!(result_matches(&result, &actual));
        actual.settings.as_mut().unwrap().goal = None;
        assert!(!result_matches(&result, &actual));
        actual.settings.as_mut().unwrap().goal = Some("1000".into());
        actual.id = "11".into();
        assert!(!result_matches(&result, &actual));
        let mut new_settings = settings();
        new_settings.goal = None;
        new_settings.icon = None;
        new_settings.end_date = None;
        let create = prepare(
            PieOperationInput::Create {
                settings: new_settings,
            },
            None,
        )
        .unwrap();
        actual.settings.as_mut().unwrap().name = Some("  Original broker name  ".into());
        assert!(result_matches(&create, &actual));
        actual.settings.as_mut().unwrap().dividend_cash_action = None;
        assert!(!result_matches(&create, &actual));
    }

    #[test]
    fn canonical_numbers_accept_exact_scientific_broker_values_without_rewriting() {
        let before = snapshot();
        let mut actual = before.clone();
        actual.settings.as_mut().unwrap().instrument_shares = Some(
            shares("6e-1", "4e-1")
                .into_iter()
                .map(|(k, v)| (k, Some(v)))
                .collect(),
        );
        actual.settings.as_mut().unwrap().goal = Some("1e3".into());
        assert!(configuration_matches(&before, &actual));
        let result = prepare(
            update(PieSettingsChanges {
                name: Some("Changed".into()),
                ..Default::default()
            }),
            Some(actual),
        )
        .unwrap();
        let PreparedPieMutation::Update { settings, .. } = result.mutation else {
            panic!()
        };
        assert_eq!(settings.instrument_shares["AAPL_US_EQ"], "6e-1");
        assert_eq!(settings.goal.as_deref(), Some("1e3"));
        assert!(!numeric_equal("1e-100", "0"));
    }

    #[test]
    fn explicit_null_is_not_an_omission() {
        for input in [
            r#"{"name":null}"#,
            r#"{"icon":null}"#,
            r#"{"goal":null}"#,
            r#"{"end_date":null}"#,
            r#"{"instrument_shares":null}"#,
            r#"{"dividend_cash_action":null}"#,
        ] {
            assert!(serde_json::from_str::<PieSettingsChanges>(input).is_err());
        }
        assert!(serde_json::from_str::<PieOperationInput>(
            r#"{"action":"duplicate","pie_id":"10","name":"Copy","icon":null}"#
        )
        .is_err());
    }
}
