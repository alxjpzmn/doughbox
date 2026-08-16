use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;

use crate::{
    database::{
        models::fund_report::FundTaxReport, queries::fund_report::add_oekb_fund_report_to_db,
    },
    services::parsers::parse_timestamp,
};

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
    InlaendischeDividenden,
    KestInlaendischeDividenden,
}

impl ReportItemType {
    fn from_id(id: i32) -> Option<ReportItemType> {
        match id {
            10286 => Some(ReportItemType::Dividends),
            10287 => Some(ReportItemType::DividendAequivalents),
            10595 => Some(ReportItemType::IntermittentDividends),
            10288 => Some(ReportItemType::WithHeldDividend),
            10289 => Some(ReportItemType::WacAdjustment),
            10759 => Some(ReportItemType::InlaendischeDividenden),
            10760 => Some(ReportItemType::KestInlaendischeDividenden),
            _ => None,
        }
    }
}
#[derive(Deserialize, Debug)]
struct OekbFundReportResponseItem {
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
struct OekbFundReportResponse {
    list: Vec<OekbFundReportResponseItem>,
}

const OEKB_HEADERS: [&str; 4] = [
    "Accept",
    "Accept-Language",
    "OeKB-Platform-Context",
    "User-Agent",
];

const OEKB_HEADER_VALUES: [&str; 4] = [
    "application/json",
    "de",
    "eyJsYW5ndWFnZSI6ImRlIiwicGxhdGZvcm0iOiJLTVMiLCJkYXNoYm9hcmQiOiJLTVNfT1VUUFVUIn0=",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.4.1 Safari/605.1.15",
];

fn build_oekb_client() -> reqwest::Client {
    reqwest::Client::new()
}

fn add_oekb_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut req = req;
    for i in 0..OEKB_HEADERS.len() {
        req = req.header(OEKB_HEADERS[i], OEKB_HEADER_VALUES[i]);
    }
    req
}

pub async fn fetch_and_store_oekb_fund_report(isin: &str) -> anyhow::Result<()> {
    let client = build_oekb_client();
    let response = add_oekb_headers(
        client.get(format!("https://my.oekb.at/fond-info/rest/public/steuerMeldung/isin/{}", isin))
    )
    .send()
    .await?;

    println!("Getting OeKB fund reports for {:?}", &isin);

    if response.status().is_success() {
        let oekb_funds_reponse_data =
            serde_json::from_str::<OekbFundReportResponse>(&response.text().await?);

        for report in oekb_funds_reponse_data?.list {
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
                inlaendische_dividenden: dec!(0),
                kest_inlaendische_dividenden: dec!(0),
                kest_per_share: dec!(0),
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
                        ReportItemType::InlaendischeDividenden => {
                            report_to_store.inlaendische_dividenden = report_item.amount
                        }
                        ReportItemType::KestInlaendischeDividenden => {
                            report_to_store.kest_inlaendische_dividenden = report_item.amount
                        }
                    },
                    None => println!("No fund type found for id {}", report_item.id),
                }
            }

            let kest_items = query_oekb_kest(report.report_id).await?;
            for item in kest_items {
                if item.id == 10105 {
                    report_to_store.kest_per_share = item.amount;
                }
            }

            add_oekb_fund_report_to_db(report_to_store).await?;
        }
    } else {
        println!("Error while getting oekb funds data: {:?}", response);
    }
    Ok(())
}

pub async fn query_oekb_fund_report(report_id: i32) -> anyhow::Result<Vec<OekbFullTaxReport>> {
    let client = build_oekb_client();
    let response = add_oekb_headers(
        client.get(format!("https://my.oekb.at/fond-info/rest/public/steuerMeldung/stmId/{}/privatAnl", &report_id))
    )
    .send()
    .await?;

    if response.status().is_success() {
        let oekb_tax_report_data =
            serde_json::from_str::<OekbFullTaxReportResponse>(&response.text().await?)?;
        Ok(oekb_tax_report_data.list)
    } else {
        Err(anyhow::anyhow!(
            "Couldn't get OeKB tax report for stmId {}: HTTP {}",
            report_id,
            response.status()
        ))
    }
}

pub async fn query_oekb_kest(report_id: i32) -> anyhow::Result<Vec<OekbFullTaxReport>> {
    let client = build_oekb_client();
    let response = add_oekb_headers(
        client.get(format!("https://my.oekb.at/fond-info/rest/public/steuerMeldung/stmId/{}/ertrStBeh", &report_id))
    )
    .send()
    .await?;

    if response.status().is_success() {
        let data =
            serde_json::from_str::<OekbFullTaxReportResponse>(&response.text().await?)?;
        Ok(data.list)
    } else {
        Err(anyhow::anyhow!(
            "Couldn't get OeKB KESt data for stmId {}: HTTP {}",
            report_id,
            response.status()
        ))
    }
}
