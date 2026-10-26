//! Public Euronext delayed cash-trade snapshots.
//!
//! The download UI at https://marketdata.euronext.com/data-reporting-service/trades-file
//! defines the product, time-selection and location path components below. It returns
//! ZIP archives, not bare CSV. Its first CSV line is a copyright/terms notice, followed
//! by the twenty-column header inspected in live EQUITIES/PAR and ETFS_FUNDS/MIL files.
//! MMT meanings: Euronext Optiq MDG Messages v5.36.0, sections 6.1 and 8 (MMT fields).
//! Trade identifiers are unique per execution MIC and trading day (Euronext CFTS EOD
//! specification v6.370.0, Trade Unique Identifier).
//!
//! SINCE_PREVIOUS_TRADING_DAY is the widest selection offered by the public UI. The
//! service promises *at least 24 hours*, not a 48-hour historical archive. Repeated
//! rolling snapshots preserve locally received reports; an outage cannot be backfilled
//! beyond what the service still exposes. Publication time, not execution time, bounds
//! the application's 48-hour ingestion window.

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Cursor, Read},
    str::FromStr,
    sync::LazyLock,
};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::{
    header::{ETAG, IF_NONE_MATCH},
    Client, StatusCode, Url,
};
use rust_decimal::Decimal;

use super::{
    config::Feed,
    http::{self, RequestLimiter},
    types::{Download, PriceReport, SourceFile},
};

const BASE_URL: &str =
    "https://marketdata.euronext.com/data-reporting-service/trades-file/download";
const TIME_SELECTION: &str = "SINCE_PREVIOUS_TRADING_DAY";
const PRODUCTS: [&str; 2] = ["EQUITIES", "ETFS_FUNDS"];
const LOCATIONS: [&str; 7] = ["AMS", "BRU", "DUB", "LIS", "MIL", "OSL", "PAR"];
const MAX_ZIP_BYTES: usize = 64 * 1024 * 1024;
const MAX_CSV_BYTES: u64 = 512 * 1024 * 1024;
const NOTICE_LIMIT: u64 = 16 * 1024;
static REQUEST_LIMITER: LazyLock<RequestLimiter> = LazyLock::new(RequestLimiter::new);

// Column order is discovered from the header, never assumed. Requiring the complete
// inspected schema prevents an HTML/error payload or changed flags from looking empty.
const COLUMNS: [&str; 20] = [
    "TradingDateTime",
    "PublicationDateTime",
    "MifidInstrumentID",
    "MifidPrice",
    "MifidQuantity",
    "MifidPriceNotation",
    "MifidCurrency",
    "MmtMarketMechanism",
    "MmtNegotiationIndicator",
    "MmtModificationIndicator",
    "MmtBenchMarkIndicator",
    "MmtContributionToPrice",
    "MmtAlgorithmicIndicator",
    "MmtPublicationMode",
    "Venue",
    "ThirdCountryTradingVenueExecution",
    "TradeUniqueIdentifier",
    "MissingPrice",
    "MmtContingentTransactionIndicator",
    "VenueOfPublication",
];

pub async fn discover(client: &Client) -> anyhow::Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    for product in PRODUCTS {
        for location in LOCATIONS {
            let url = Url::parse(&format!("{BASE_URL}/{product}/{TIME_SELECTION}/{location}"))?;
            // The actual UI uses this HEAD request: 404 means "No trade is found".
            // In particular, ETF selections for Dublin/Lisbon legitimately return 404.
            let response = REQUEST_LIMITER.send(client.head(url.clone())).await?;
            if response.status() == StatusCode::NOT_FOUND {
                continue;
            }
            http::check_status(&response)?;
            files.push(SourceFile {
                key: format!("EURONEXT-{product}-{TIME_SELECTION}-{location}"),
                // A rolling query is mutable forever. Using its current coverage end
                // also ensures the shared checkpoint layer revalidates it every refresh.
                timestamp: Utc::now(),
                url,
            });
        }
    }
    files.sort_by(|a, b| a.key.cmp(&b.key));
    files.dedup_by(|a, b| a.key == b.key);
    Ok(files)
}

pub async fn download(
    client: &Client,
    file: &SourceFile,
    etag: Option<&str>,
) -> anyhow::Result<Option<Download>> {
    let mut request = client.get(file.url.clone());
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = REQUEST_LIMITER.send(request).await?;
    // Data can roll out of the live selection between discovery and GET.
    if matches!(
        response.status(),
        StatusCode::NOT_MODIFIED | StatusCode::NOT_FOUND
    ) {
        return Ok(None);
    }
    http::check_status(&response)?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = http::limited_body(response, MAX_ZIP_BYTES).await?;
    Ok(Some(Download { bytes, etag }))
}

pub fn parse(
    file: &SourceFile,
    bytes: &[u8],
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    parse_archive(bytes, tracked, now)
        .with_context(|| format!("Parsing Euronext file {}", file.key))
}

fn parse_archive(
    bytes: &[u8],
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    ensure!(
        bytes.len() <= MAX_ZIP_BYTES,
        "Euronext archive exceeds size limit"
    );
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("Invalid Euronext ZIP archive")?;
    ensure!(archive.len() == 1, "Unexpected Euronext ZIP members");
    let member = archive
        .by_index(0)
        .context("Cannot open Euronext CSV member")?;
    ensure!(
        matches!(
            member.name(),
            "Trades_Equities.csv" | "Trades_ETFSFunds.csv"
        ),
        "Unexpected Euronext CSV member name"
    );
    ensure!(
        member.size() <= MAX_CSV_BYTES,
        "Euronext CSV exceeds size limit"
    );
    // Stream the inflated member: the inspected Paris CSV alone is 98 MiB.
    let mut reader = BufReader::new(member.take(MAX_CSV_BYTES + 1));
    let mut notice = Vec::new();
    (&mut reader)
        .take(NOTICE_LIMIT)
        .read_until(b'\n', &mut notice)?;
    ensure!(
        notice.ends_with(b"\n"),
        "Missing Euronext terms notice terminator"
    );
    let notice = std::str::from_utf8(&notice).context("Invalid Euronext terms notice")?;
    ensure!(
        notice.trim_start_matches('\u{feff}').starts_with("(c) ")
            && notice.contains("Euronext N.V.")
            && notice.contains("Terms and Conditions for Delayed Trade Data"),
        "Missing Euronext terms notice"
    );
    parse_csv(reader, tracked, now)
}

fn parse_csv(
    reader: impl Read,
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    let mut csv = csv::ReaderBuilder::new().from_reader(reader);
    let headers = csv
        .headers()
        .context("Missing Euronext CSV header")?
        .clone();
    let mut positions = [0usize; 20];
    for (i, name) in COLUMNS.iter().enumerate() {
        let mut matches = headers
            .iter()
            .enumerate()
            .filter(|(_, field)| field == name);
        positions[i] = matches
            .next()
            .with_context(|| format!("Missing Euronext column {name}"))?
            .0;
        ensure!(matches.next().is_none(), "Duplicate Euronext column {name}");
    }
    let additional_flags: Vec<usize> = headers
        .iter()
        .enumerate()
        .filter(|(_, name)| name.starts_with("Mmt") && !COLUMNS.contains(name))
        .map(|(index, _)| index)
        .collect();
    let mut reports = Vec::new();
    let mut row = csv::StringRecord::new();
    let cutoff = now - Duration::hours(48);
    let mut index = 1usize;
    while csv
        .read_record(&mut row)
        .with_context(|| format!("Invalid Euronext CSV row {}", index + 1))?
    {
        index += 1;
        let value = |column: usize| row.get(positions[column]).unwrap_or_default();
        let isin = value(2);
        if !tracked.contains(isin) {
            continue;
        }
        let published = timestamp(value(1))
            .with_context(|| format!("Invalid publication time at Euronext row {index}"))?;
        if published < cutoff || published > now {
            continue;
        }
        let executed = timestamp(value(0))
            .with_context(|| format!("Invalid trading time at Euronext row {index}"))?;
        let revision = match value(9) {
            "-" => 0,
            "AMND" => 1,
            "CANC" => 2,
            _ => bail!("Unknown Euronext modification indicator at row {index}"),
        };
        let venue = value(14);
        let publisher = value(19);
        let transaction = value(16);
        ensure!(
            !venue.is_empty() && !publisher.is_empty() && !transaction.is_empty(),
            "Missing Euronext transaction identity at row {index}"
        );
        let price = Decimal::from_str(value(3)).ok();
        let quantity = Decimal::from_str(value(4)).ok();
        // CSV MONE values already contain the decimal point: no binary SBE scaling.
        // Preserve ineligible corrections/cancellations so they invalidate an earlier
        // valuation with the same identity, including currency/condition amendments.
        let eligible = revision != 2
            && value(6) == "EUR"
            && value(5) == "MONE"
            && price.is_some_and(|p| p > Decimal::ZERO)
            && quantity.is_some_and(|q| q >= Decimal::ZERO)
            && executed <= now
            && venue == publisher
            && !matches!(venue, "XOFF" | "SINT")
            && value(15).is_empty()
            && value(17).is_empty()
            && matches!(value(7), "1" | "2" | "3" | "5")
            && value(8) == "-"
            && value(10) == "-"
            && matches!(value(11), "-" | "P")
            && matches!(value(12), "-" | "ALGO")
            && value(13) == "-"
            && value(18) == "-"
            && additional_flags.iter().all(|&i| row.get(i) == Some("-"));
        let identity = format!(
            "{}|{venue}|{}|{transaction}",
            Feed::Euronext.key(),
            executed.date_naive()
        );
        let mut revision_hash = blake3::Hasher::new();
        // Hash every provider field, with boundaries, including unfamiliar columns.
        // The hash is independent of ZIP timestamps and CSV quoting differences.
        for field in row.iter() {
            revision_hash.update(&(field.len() as u64).to_le_bytes());
            revision_hash.update(field.as_bytes());
        }
        reports.push(PriceReport {
            id: blake3::hash(identity.as_bytes()).to_hex().to_string(),
            isin: isin.to_owned(),
            transaction_id: transaction.to_owned(),
            publisher: publisher.to_owned(),
            venue: venue.to_owned(),
            executed_at: executed,
            executed_ns: executed
                .timestamp_nanos_opt()
                .context("Euronext execution timestamp out of range")?,
            published_ns: published
                .timestamp_nanos_opt()
                .context("Euronext publication timestamp out of range")?,
            revision,
            revision_hash: revision_hash.finalize().to_hex().to_string(),
            price: price.unwrap_or(Decimal::ZERO),
            currency: value(6).to_owned(),
            eligible,
        });
    }
    Ok(reports)
}

fn timestamp(value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Actual first EQUITIES/SINCE_PREVIOUS_TRADING_DAY/PAR record, downloaded
    // 2026-10-25. Tests below vary individual published fields, not invented schemas.
    const REAL_ROW: &str = "2026-10-23T07:00:00.049194Z,2026-10-23T07:00:00.050344Z,FR0004056851,2.4000000,42.0,MONE,EUR,1,-,-,-,-,-,-,XPAR,,1OO7IL76Y,,-,XPAR";

    fn now() -> DateTime<Utc> {
        timestamp("2026-10-23T20:00:00Z").unwrap()
    }
    fn tracked() -> HashSet<String> {
        HashSet::from(["FR0004056851".to_owned()])
    }
    fn fields() -> Vec<String> {
        REAL_ROW.split(',').map(str::to_owned).collect()
    }
    fn csv_bytes(rows: &[Vec<String>]) -> Vec<u8> {
        let mut csv = csv::Writer::from_writer(Vec::new());
        csv.write_record(COLUMNS).unwrap();
        for row in rows {
            csv.write_record(row).unwrap();
        }
        csv.into_inner().unwrap()
    }
    fn report(fields: Vec<String>) -> PriceReport {
        parse_csv(Cursor::new(csv_bytes(&[fields])), &tracked(), now())
            .unwrap()
            .pop()
            .unwrap()
    }

    #[test]
    fn real_monetary_price_is_not_rescaled_and_zero_quantity_is_valid() {
        let original = report(fields());
        assert!(original.eligible);
        assert_eq!(original.price, Decimal::from_str("2.4").unwrap());
        let mut zero = fields();
        zero[4] = "0.0".into();
        assert!(report(zero).eligible);
        for quantity in ["-1", "", "NaN"] {
            let mut row = fields();
            row[4] = quantity.into();
            assert!(!report(row).eligible);
        }
    }

    #[test]
    fn amendments_and_cancellations_keep_original_identity() {
        let original = report(fields());
        let mut amendment = fields();
        amendment[1] = "2026-10-23T08:00:00Z".into();
        amendment[9] = "AMND".into();
        amendment[6] = "USD".into();
        let amendment = report(amendment);
        assert_eq!(original.id, amendment.id);
        assert_eq!(amendment.revision, 1);
        assert!(!amendment.eligible);
        assert_ne!(original.revision_hash, amendment.revision_hash);
        let mut cancellation = fields();
        cancellation[9] = "CANC".into();
        cancellation[3].clear();
        cancellation[4].clear();
        let cancellation = report(cancellation);
        assert_eq!(original.id, cancellation.id);
        assert_eq!(cancellation.revision, 2);
        assert!(!cancellation.eligible);
    }

    #[test]
    fn identity_is_scoped_by_execution_mic_and_trading_day() {
        let original = report(fields());
        let mut other_venue = fields();
        other_venue[14] = "ALXP".into();
        other_venue[19] = "ALXP".into();
        assert_ne!(original.id, report(other_venue).id);
        let mut other_day = fields();
        other_day[0] = "2026-10-22T07:00:00Z".into();
        assert_ne!(original.id, report(other_day).id);
    }

    #[test]
    fn rejects_non_price_forming_apa_and_non_monetary_valuations() {
        for (column, value) in [
            (3, "0"),
            (3, "-1"),
            (5, "PERC"),
            (6, "NOK"),
            (7, "4"),
            (7, "8"),
            (8, "NLIQ"),
            (10, "RFPT"),
            (11, "NPFT"),
            (11, "PNDG"),
            (13, "NIPM"),
            (14, "XOFF"),
            (15, "XNAS"),
            (17, "NOAP"),
            (18, "CONT"),
            (19, "APA1"),
            (12, "UNKNOWN"),
        ] {
            let mut row = fields();
            row[column] = value.into();
            assert!(!report(row).eligible, "column {column}: {value}");
        }
        let mut vanilla = fields();
        vanilla[11] = "P".into();
        vanilla[12] = "ALGO".into();
        assert!(report(vanilla).eligible);
    }

    #[test]
    fn retention_filters_publication_not_original_execution() {
        let mut correction = fields();
        correction[0] = "2026-10-01T07:00:00Z".into();
        correction[9] = "AMND".into();
        assert_eq!(report(correction).revision, 1);
        let mut old = fields();
        old[1] = "2026-10-21T19:59:59Z".into();
        assert!(parse_csv(Cursor::new(csv_bytes(&[old])), &tracked(), now())
            .unwrap()
            .is_empty());
        let mut boundary = fields();
        boundary[1] = "2026-10-21T20:00:00Z".into();
        boundary[0] = boundary[1].clone();
        assert_eq!(
            parse_csv(Cursor::new(csv_bytes(&[boundary])), &tracked(), now())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn schema_and_unknown_revisions_fail_instead_of_succeeding_empty() {
        assert!(parse_csv(
            Cursor::new(b"TradingDateTime,MifidInstrumentID\n"),
            &tracked(),
            now()
        )
        .is_err());
        let mut row = fields();
        row[9] = "UNKN".into();
        assert!(parse_csv(Cursor::new(csv_bytes(&[row])), &tracked(), now()).is_err());
    }

    #[test]
    fn header_order_is_not_assumed_and_unknown_flags_fail_closed() {
        let mut headers: Vec<&str> = COLUMNS.into_iter().collect();
        headers.push("MmtNewCondition");
        let mut row = fields();
        row.push("SPECIAL".into());
        headers.reverse();
        row.reverse();
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.write_record(&headers).unwrap();
        writer.write_record(&row).unwrap();
        let bytes = writer.into_inner().unwrap();
        let reports = parse_csv(Cursor::new(bytes), &tracked(), now()).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].eligible);
        assert_eq!(reports[0].price, Decimal::from_str("2.4").unwrap());
    }

    #[test]
    fn row_hash_is_deterministic_and_includes_non_identity_fields() {
        let first = report(fields());
        assert_eq!(first.revision_hash, report(fields()).revision_hash);
        let mut changed = fields();
        changed[4] = "43.0".into();
        let changed = report(changed);
        assert_eq!(first.id, changed.id);
        assert_ne!(first.revision_hash, changed.revision_hash);
    }

    #[test]
    fn future_revisions_cannot_invalidate_a_current_valuation() {
        for modification in ["-", "AMND", "CANC"] {
            let mut future = fields();
            future[1] = "2026-10-24T07:00:00Z".into();
            future[9] = modification.into();
            let reports = parse_csv(
                Cursor::new(csv_bytes(&[fields(), future])),
                &tracked(),
                now(),
            )
            .unwrap();
            assert_eq!(reports.len(), 1);
            assert!(reports[0].eligible);
            assert_eq!(reports[0].revision, 0);
        }
        assert!(
            parse_csv(Cursor::new(csv_bytes(&[fields()])), &HashSet::new(), now())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn actual_zip_layout_and_terms_notice_are_required() {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "Trades_Equities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"(c) 2025 Euronext N.V. All Rights Reserved. Terms and Conditions for Delayed Trade Data\n").unwrap();
        writer.write_all(&csv_bytes(&[fields()])).unwrap();
        let zip = writer.finish().unwrap().into_inner();
        let reports = parse_archive(&zip, &tracked(), now()).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].eligible);
        assert!(parse_archive(b"<html>Access denied</html>", &tracked(), now()).is_err());
    }
}
