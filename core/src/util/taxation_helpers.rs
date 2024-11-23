use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tokio::try_join;
use tokio_postgres::{Client, Row};

use crate::util::db_helpers::add_fund_report_to_db;

use super::{
    db_helpers::{db_client, get_listing_changes, get_stock_splits, FundTaxReport},
    general_helpers::parse_timestamp,
    market_data_helpers::{
        get_changed_identifier, get_split_adjusted_price_per_unit, get_split_adjusted_units,
    },
};

#[derive(Debug, Clone, Serialize)]
pub enum TaxEventType {
    CashInterest,
    ShareInterest,
    Dividend,
    Trade,
    FxConversion,
    DividendAequivalent,
}

#[derive(Debug, Clone, Serialize)]
pub enum TradeDirection {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxRelevantEvent {
    pub date: DateTime<Utc>,
    pub event_type: TaxEventType,
    pub currency: String,
    pub units: Decimal,
    pub price_unit: Decimal,
    pub identifier: Option<String>,
    pub direction: Option<TradeDirection>,
    pub applied_fx_rate: Option<Decimal>,
    pub withholding_tax_percent: Option<Decimal>,
}

pub async fn get_tax_relevant_events(year: i32) -> anyhow::Result<Vec<TaxRelevantEvent>> {
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

pub async fn get_events(
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let client = db_client().await?;

    let (interest_rows, fund_report_rows, dividend_rows, trade_rows, fx_conversion_rows) = try_join!(
        query_interest(&client, &start_date, &end_date),
        query_fund_reports(&client, &start_date, &end_date),
        query_dividends(&client, &start_date, &end_date),
        query_trades(&client, &start_date, &end_date),
        query_fx_conversions(&client, &start_date, &end_date)
    )?;

    let mut events = Vec::new();
    events.extend(process_interest_rows(interest_rows)?);
    events.extend(process_fund_report_rows(fund_report_rows)?);
    events.extend(process_dividend_rows(dividend_rows)?);
    events.extend(process_trade_rows(trade_rows).await?);
    events.extend(process_fx_conversion_rows(fx_conversion_rows)?);

    events.sort_by(|event_a, event_b| event_a.date.cmp(&event_b.date));

    Ok(events)
}

async fn query_interest(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT date, amount, currency, principal, withholding_tax, witholding_tax_currency, amount_eur FROM interest WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_fund_reports(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT date, id, currency FROM fund_reports WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_dividends(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT date, amount, currency, isin, withholding_tax, witholding_tax_currency, amount_eur FROM dividends WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_trades(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT date, no_units, avg_price_per_unit, currency_denomination, isin, direction, withholding_tax, witholding_tax_currency, eur_avg_price_per_unit FROM trades WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_fx_conversions(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT date, from_amount, to_amount, from_currency, to_currency FROM fx_conversions WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

fn process_interest_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(4) != dec!(0.0)
            && row.get::<usize, String>(5) != row.get::<usize, String>(2)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, Decimal>(1)
            )
        }

        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: if row.get::<usize, String>(3) == "Cash" {
                TaxEventType::CashInterest
            } else {
                TaxEventType::ShareInterest
            },
            identifier: None,
            units: row.get(1),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: Some(row.get::<usize, Decimal>(1) / row.get::<usize, Decimal>(6)),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0))
                    / row.get::<usize, Decimal>(1),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fund_report_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::DividendAequivalent,
            identifier: Some(row.get::<usize, i32>(1).to_string()),
            units: dec!(1.00),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: None,
            withholding_tax_percent: None,
        };
        events.push(event);
    }
    Ok(events)
}

fn process_dividend_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(4) != dec!(0.0)
            && row.get::<usize, String>(5) != row.get::<usize, String>(2)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, String>(3)
            )
        }
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::Dividend,
            identifier: row.get(3),
            units: row.get(1),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: Some(row.get::<usize, Decimal>(1) / row.get::<usize, Decimal>(6)),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0))
                    / row.get::<usize, Decimal>(1),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

async fn process_trade_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut stock_split_information = get_stock_splits().await?;
    let listing_changes = get_listing_changes().await?;

    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(6) != dec!(0.0)
            && row.get::<usize, String>(7) != row.get::<usize, String>(3)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, String>(4)
            )
        }

        let split_adjusted_units = get_split_adjusted_units(
            row.get(4),
            row.get(1),
            row.get(0),
            &mut stock_split_information,
        );
        let split_adjusted_price_per_unit = get_split_adjusted_price_per_unit(
            row.get(4),
            row.get(2),
            row.get(0),
            &mut stock_split_information,
        );
        let isin = get_changed_identifier(row.get(4), listing_changes.clone());

        let applied_fx_rate = if row.get::<usize, Decimal>(2) == dec!(0.0) {
            dec!(1)
        } else {
            row.get::<usize, Decimal>(2)
        } / if row.get::<usize, Decimal>(8) == dec!(0.0) {
            dec!(1.0)
        } else {
            row.get::<usize, Decimal>(8)
        };

        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::Trade,
            identifier: Some(isin),
            units: split_adjusted_units,
            price_unit: split_adjusted_price_per_unit,
            currency: row.get(3),
            direction: Some(if row.get::<usize, String>(5) == *"Buy" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(applied_fx_rate),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(6).unwrap_or(dec!(0.00))
                    / (row.get::<usize, Decimal>(1)
                        * if row.get::<usize, Decimal>(2) == dec!(0.0) {
                            dec!(1)
                        } else {
                            row.get::<usize, Decimal>(2)
                        }),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fx_conversion_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::FxConversion,
            currency: row.get(3),
            identifier: Some(format!(
                "{}{}",
                row.get::<usize, String>(3),
                row.get::<usize, String>(4)
            )),
            direction: Some(if row.get::<usize, String>(3) == *"EUR" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1)),
            units: row.get(1),
            price_unit: row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1),
            withholding_tax_percent: None,
        };
        events.push(event);
    }
    Ok(events)
}

pub async fn get_tax_relevant_years() -> anyhow::Result<Vec<i32>> {
    let client = db_client().await?;

    let mut years: Vec<i32> = vec![];

    let rows = client
        .query(
            "WITH all_dates AS (
                SELECT MIN(date) AS earliest_date FROM (
                SELECT date FROM interest
                UNION ALL
                SELECT date FROM trades
                UNION ALL
                SELECT date FROM fx_conversions
                UNION ALL
                SELECT date FROM dividends
                ) AS all_dates
            )
            SELECT 
            GENERATE_SERIES(EXTRACT(YEAR FROM earliest_date)::INT, EXTRACT(YEAR FROM CURRENT_DATE)::INT) AS years
            FROM all_dates;
            ",
            &[],
        )
        .await?;

    for row in rows {
        let year = row.get::<usize, i32>(0);
        years.push(year);
    }

    Ok(years)
}

#[derive(Deserialize, Debug)]
struct OekbTaxReport {
    #[serde(alias = "stmId")]
    report_id: i32,
    #[serde(alias = "waehrung")]
    currency: String,
    #[serde(alias = "gjEnde")]
    _period_end_date: String,
    #[serde(alias = "gjBeginn")]
    _period_start_date: String,
    #[serde(alias = "zufluss")]
    report_date: String,
    #[serde(alias = "gueltAb")]
    _valid_from: String,
    isin: String,
}

#[derive(Deserialize, Debug)]
struct OekbFundsDateResponse {
    list: Vec<OekbTaxReport>,
}

pub async fn query_for_oekb_funds_data(isin: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("https://my.oekb.at/fond-info/rest/public/steuerMeldung/isin/{}", isin))
        .header("Accept", "application/json")
        .header("Accept-Language", "de")
        .header("OeKB-Platform-Context",
          "eyJsYW5ndWFnZSI6ImRlIiwicGxhdGZvcm0iOiJLTVMiLCJkYXNoYm9hcmQiOiJLTVNfT1VUUFVUIn0=")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4.1 Safari/605.1.15")
        .send()
        .await?;

    println!("Getting OeKB fund reports for {:?}", &isin);

    if response.status().is_success() {
        let oekb_funds_reponse_data =
            serde_json::from_str::<OekbFundsDateResponse>(&response.text().await?);

        for report in oekb_funds_reponse_data?.list {
            // now for each item in the list, make call to get the actual report
            let mut report_to_store = FundTaxReport {
                id: report.report_id,
                date: parse_timestamp(report.report_date.as_str())?,
                isin: report.isin,
                currency: report.currency,
                dividend: dec!(0),
                dividend_aequivalent: dec!(0),
                intermittent_dividends: dec!(0),
                withheld_dividend: dec!(0),
                wac_adjustment: dec!(0),
            };

            let report_items = query_oekb_fund_report(report.report_id).await?;
            for report_item in report_items {
                let fund_type = ReportItemType::from_id(report_item.id);
                match fund_type {
                    Some(fund_type) => match fund_type {
                        ReportItemType::Dividends => report_to_store.dividend = report_item.amount,
                        ReportItemType::DividendAequivalents => {
                            report_to_store.dividend_aequivalent = report_item.amount
                        }
                        ReportItemType::IntermittentDividends => {
                            report_to_store.intermittent_dividends = report_item.amount
                        }
                        ReportItemType::WithHeldDividend => {
                            report_to_store.withheld_dividend = report_item.amount
                        }
                        ReportItemType::WacAdjustment => {
                            report_to_store.wac_adjustment = report_item.amount
                        }
                    },
                    None => println!("No fund type found for id {}", report_item.id),
                }
            }
            add_fund_report_to_db(report_to_store).await?;
        }
    } else {
        println!("error while getting oekb funds data: {:?}", response);
    }
    Ok(())
}

#[derive(Deserialize, Debug)]
pub struct OekbFullTaxReport {
    #[serde(alias = "steuerCode")]
    id: i32,
    #[serde(alias = "pvMitOption4")]
    amount: Decimal,
}

#[derive(Deserialize, Debug)]
struct OekbFullTaxReportResponse {
    list: Vec<OekbFullTaxReport>,
}

#[derive(Debug)]
enum ReportItemType {
    Dividends,
    DividendAequivalents,
    IntermittentDividends,
    WithHeldDividend,
    WacAdjustment,
}

impl ReportItemType {
    fn from_id(id: i32) -> Option<ReportItemType> {
        match id {
            10286 => Some(ReportItemType::Dividends),
            10287 => Some(ReportItemType::DividendAequivalents),
            10595 => Some(ReportItemType::IntermittentDividends),
            10288 => Some(ReportItemType::WithHeldDividend),
            10289 => Some(ReportItemType::WacAdjustment),
            _ => None,
        }
    }
}

pub async fn query_oekb_fund_report(report_id: i32) -> anyhow::Result<Vec<OekbFullTaxReport>> {
    let client = reqwest::Client::new();
    let response = client
    .get(format!("https://my.oekb.at/fond-info/rest/public/steuerMeldung/stmId/{}/privatAnl", &report_id))
    .header("Accept", "application/json")
    .header("Accept-Language", "de")
    .header("OeKB-Platform-Context",
    "eyJsYW5ndWFnZSI6ImRlIiwicGxhdGZvcm0iOiJLTVMiLCJkYXNoYm9hcmQiOiJLTVNfT1VUUFVUIn0=")
    .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4.1 Safari/605.1.15")
    .send()
    .await?;

    if response.status().is_success() {
        let oekb_tax_report_data =
            serde_json::from_str::<OekbFullTaxReportResponse>(&response.text().await?)?;
        Ok(oekb_tax_report_data.list)
    } else {
        panic!("Couldn't get OeKB tax report.")
    }
}
