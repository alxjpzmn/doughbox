use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;

use crate::{
    database::{models::fund_report::FundTaxReport, queries::fund_report::add_fund_report_to_db},
    services::parsers::parse_timestamp,
};

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
