use super::general_helpers::does_match_exist;

#[derive(Debug)]
pub enum Broker {
    TradeRepublic,
    Revolut,
    Lightyear,
    InteractiveBrokers,
    Scalable,
    ErsteBank,
    Trading212,
    Wise,
    Manual,
}

pub fn detect_broker_from_csv_header(record: &csv::StringRecord) -> anyhow::Result<Option<Broker>> {
    if record.get(0).unwrap().contains("Action") {
        return Ok(Some(Broker::Trading212));
    }
    if record.get(0).unwrap() == "Traded Asset ID Type"
        || record.get(0).unwrap() == "TransferWise ID"
    {
        return Ok(Some(Broker::Wise));
    }
    if (record.get(0).unwrap() == "Date" && record.get(1).unwrap() == "Ticker")
        || record.get(1).unwrap() == "Product"
    {
        return Ok(Some(Broker::Revolut));
    }
    if record.get(0).unwrap() == "Date" && record.get(1).unwrap() == "Reference" {
        return Ok(Some(Broker::Lightyear));
    }
    if record.get(0).unwrap() == "ClientAccountID" {
        return Ok(Some(Broker::InteractiveBrokers));
    }
    if record.get(0).unwrap() == "date" && record.get(4).unwrap() == "direction" {
        return Ok(Some(Broker::Manual));
    }
    Ok(None)
}

pub fn detect_broker_from_pdf_text(text: &str) -> Option<Broker> {
    if does_match_exist(r"TRADE REPUBLIC BANK GMBH", text) {
        return Some(Broker::TradeRepublic);
    }
    if does_match_exist(r"Erste Bank", text) {
        return Some(Broker::ErsteBank);
    }
    if does_match_exist(r"Scalable", text) {
        return Some(Broker::Scalable);
    }
    None
}
