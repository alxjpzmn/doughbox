use crate::{
    services::importers::{
        erste_bank::extract_erste_bank_record, ibkr::extract_ibkr_record,
        lightyear::extract_lightyear_record, manual::extract_manual_record,
        revolut::extract_revolut_record, scalable::extract_scalable_record,
        trade_republic::extract_trade_republic_record, trading212::extract_trading212_record,
        wise::extract_wise_record,
    },
    util::{
        broker_helpers::{detect_broker_from_csv_header, detect_broker_from_pdf_text, Broker},
        import_helpers::{detect_file_format, FileFormat},
    },
};
use deunicode::deunicode;
use walkdir::WalkDir;

pub async fn import(directory_path: &str) -> anyhow::Result<()> {
    for entry in WalkDir::new(directory_path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path().to_str().unwrap();
        let file_format = detect_file_format(entry_path);

        match file_format {
            FileFormat::Pdf => {
                let bytes = std::fs::read(entry.path())?;

                let mut text = pdf_extract::extract_text_from_mem(&bytes)?;
                text = deunicode(&text).replace('\0', "").replace("[?]", "");

                let broker = detect_broker_from_pdf_text(&text);

                match broker {
                    Some(Broker::TradeRepublic) => {
                        extract_trade_republic_record(&text, entry_path).await?;
                    }
                    Some(Broker::Scalable) => {
                        extract_scalable_record(&text, entry_path).await?;
                    }
                    Some(Broker::ErsteBank) => {
                        extract_erste_bank_record(&text, entry_path).await?;
                    }
                    Some(_) => panic!("Broker wrongly matched"),
                    None => println!("No broker matched"),
                }
            }
            FileFormat::Csv => {
                let mut rdr = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_path(entry_path)?;

                let broker = detect_broker_from_csv_header(rdr.headers()?)?;

                match broker {
                    Some(Broker::Trading212) => {
                        extract_trading212_record(entry_path).await?;
                    }
                    Some(Broker::Revolut) => {
                        extract_revolut_record(entry_path).await?;
                    }
                    Some(Broker::Lightyear) => {
                        extract_lightyear_record(entry_path).await?;
                    }
                    Some(Broker::InteractiveBrokers) => {
                        extract_ibkr_record(entry_path).await?;
                    }
                    Some(Broker::Wise) => {
                        extract_wise_record(entry_path).await?;
                    }
                    Some(Broker::Manual) => {
                        extract_manual_record(entry_path).await?;
                    }
                    Some(_) => panic!("Broker wrongly matched"),
                    None => println!("No broker matched"),
                }
            }
            _ => {
                println!("Skipping {:?}", entry_path)
            }
        }
    }
    Ok(())
}
