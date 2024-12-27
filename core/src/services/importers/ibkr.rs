use csv::ReaderBuilder;
use rust_decimal_macros::dec;
use std::io::Cursor;

use crate::{
    database::{
        models::{fx_conversion::FxConversion, trade::Trade},
        queries::{composite::add_trade_to_db, fx_conversion::add_fx_conversion_to_db},
    },
    services::parsers::parse_timestamp,
};
use chrono::prelude::*;
use itertools::Itertools;
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct IBRKRecord {
    _client_account_id: String,
    _account_alias: String,
    _model: String,
    currency_primary: String,
    fx_rate_to_base: String,
    asset_class: String,
    _symbol: String,
    _description: String,
    _conid: String,
    _security_id: String,
    _security_id_type: String,
    _cusip: String,
    isin: String,
    _listing_exchange: String,
    _underlying_conid: String,
    _underlying_symbol: String,
    _underlying_security_id: String,
    _underlying_listing_exchange: String,
    _issuer: String,
    _multiplier: String,
    _strike: String,
    _expiry: String,
    trade_id: String,
    _put_v_call: String,
    _related_trade_id: String,
    _principal_adjust_factor: String,
    _report_date: String,
    date_time: String,
    _trade_date: String,
    _settle_date_target: String,
    _transaction_type: String,
    _exchange: String,
    quantity: String,
    trade_price: String,
    _trade_money: String,
    proceeds: String,
    taxes: String,
    ib_commission: String,
    _ib_commission_currency: String,
    _net_cash: String,
    _close_price: String,
    _open_close_indicator: String,
    _notes_codes: String,
    _cost_basis: String,
    _fifo_pnl_realized: String,
    _fx_pnl: String,
    _mtm_pnl: String,
    _orig_trade_price: String,
    _orig_trade_date: String,
    _orig_trade_id: String,
    _orig_order_id: String,
    _clearing_firm_id: String,
    buy_sell: String,
    _transaction_id: String,
    _ib_order_id: String,
    _related_transaction_id: String,
    _ib_exec_id: String,
    _brokerage_order_id: String,
    _order_reference: String,
    _volatility_order_link: String,
    _exch_order_id: String,
    _ext_exec_id: String,
    _order_time: String,
    _open_date_time: String,
    _holding_period_date_time: String,
    _when_realized: String,
    _when_reopened: String,
    _level_of_detail: String,
    _change_in_price: String,
    _change_in_quantity: String,
    _order_type: String,
    _trader_id: String,
    _is_api_order: String,
    _accrued_interest: String,
    _serial_number: String,
    _delivery_type: String,
    _commodity_type: String,
    _fineness: String,
    _weight: String,
}

enum RecordType {
    EquityTrade,
    FxConversion,
    Unmatched,
}

fn detect_record_type(record: &IBRKRecord) -> RecordType {
    if record.asset_class == "CASH" {
        return RecordType::FxConversion;
    }
    if record.asset_class == "STK" {
        return RecordType::EquityTrade;
    }
    RecordType::Unmatched
}

pub async fn extract_ibkr_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Interactive Brokers".to_string();

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

    for result in rdr.deserialize() {
        let record: IBRKRecord = result?;

        let record_type = detect_record_type(&record);

        match record_type {
            RecordType::EquityTrade => {
                let trade = Trade {
                    broker: broker.clone(),
                    date: parse_timestamp(&record.date_time)?,
                    isin: record.isin,
                    avg_price_per_unit: record.trade_price.parse::<Decimal>()?,
                    eur_avg_price_per_unit: record.trade_price.parse::<Decimal>()?
                        * record.fx_rate_to_base.parse::<Decimal>()?,
                    // IBKR assigns negative units on sell events
                    no_units: record.quantity.parse::<Decimal>()?.abs(),
                    direction: if record.buy_sell.contains("BUY") {
                        "Buy".to_string()
                    } else {
                        "Sell".to_string()
                    },
                    security_type: "Equity".to_string(),
                    currency_denomination: record.currency_primary.to_string(),
                    date_added: Utc::now(),
                    fees: record.ib_commission.parse::<Decimal>()?
                        * dec!(-1.0)
                        * record.fx_rate_to_base.parse::<Decimal>()?,
                    withholding_tax: record.taxes.parse::<Decimal>()?,
                    witholding_tax_currency: record.currency_primary,
                };
                add_trade_to_db(trade, Some(record.trade_id)).await?;
            }
            RecordType::FxConversion => {
                let currencies = record._symbol.split('.').collect_vec();

                let fx_conversion = FxConversion {
                    date: parse_timestamp(&record.date_time)?,
                    broker: broker.clone(),
                    from_amount: if record.buy_sell.contains("SELL") {
                        record.quantity.parse::<Decimal>()? * dec!(-1.0)
                    } else {
                        record.proceeds.parse::<Decimal>()? * dec!(-1.0)
                    },
                    to_amount: if record.buy_sell.contains("SELL") {
                        record.proceeds.parse::<Decimal>()?
                    } else {
                        record.quantity.parse::<Decimal>()?
                    },
                    from_currency: if record.buy_sell.contains("SELL") {
                        currencies[0].to_string()
                    } else {
                        currencies[1].to_string()
                    },
                    to_currency: if record.buy_sell.contains("SELL") {
                        currencies[1].to_string()
                    } else {
                        currencies[0].to_string()
                    },
                    date_added: Utc::now(),
                    fees: if record
                        .ib_commission
                        .parse::<Decimal>()
                        .unwrap_or(dec!(-0.0))
                        < dec!(0.0)
                    {
                        record
                            .ib_commission
                            .parse::<Decimal>()
                            .unwrap_or(dec!(-0.0))
                            * dec!(-1.0)
                            * record.fx_rate_to_base.parse::<Decimal>()?
                    } else {
                        record
                            .ib_commission
                            .parse::<Decimal>()
                            .unwrap_or(dec!(-0.0))
                            * record.fx_rate_to_base.parse::<Decimal>()?
                    },
                };
                add_fx_conversion_to_db(fx_conversion).await?;
            }
            RecordType::Unmatched => continue,
        }
    }
    Ok(())
}
