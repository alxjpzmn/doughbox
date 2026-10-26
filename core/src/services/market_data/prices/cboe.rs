//! Cboe Europe delayed transparency CSVs (not the deprecated trade-data service).
//!
//! Discovery: https://www.cboe.com/markets/eu/equities/transparency-data/
//! Schema: Cboe Titanium Europe Delayed Transparency Files Specification 1.0.7,
//! https://cdn.cboe.com/resources/participant_resources/Cboe_Titanium_Europe_Aggregated_Depth_Feed_Specification.pdf
//! Equities/trade selectors BXE, CXE and DXE include exchange off-book and BIDS
//! executions; TRF is APA reporting and is deliberately not requested.
use std::{collections::HashSet, str::FromStr, sync::LazyLock};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use reqwest::{Client, Url};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::{
    config::Feed,
    http::{self, RequestLimiter},
    types::{Download, PriceReport, SourceFile},
};

const API: &str = "https://www-api.cboe.com/europe/equities/transparency_data/get_file_map";
const MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;
// A real DXE daily sample contained 2,232,209 rows / 258,008,506 bytes.
const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024;
const MAX_ROWS: usize = 6_000_000;
const HEADER: &str = "UpdateTime,InstrumentID,Price,PriceCurrency,PriceNotation,Quantity,VenueOfExecution,TradingSystem,PublicationTime,VenueOfPublication,TransactionID,Flags";
static LIMITER: LazyLock<RequestLimiter> = LazyLock::new(RequestLimiter::new);

#[derive(Deserialize)]
struct FileMap {
    daily: Vec<ListedFile>,
    minute: Vec<ListedFile>,
}

#[derive(Deserialize)]
struct ListedFile {
    filename: String,
    #[serde(rename = "serveUrl")]
    serve_url: String,
}

struct Bucket {
    stream: &'static str,
    day: NaiveDate,
    daily: bool,
    end: DateTime<Utc>,
}

fn bucket(key: &str) -> anyhow::Result<Bucket> {
    ensure!(key.len() <= 64 && key.is_ascii(), "Invalid Cboe file key");
    let parts: Vec<_> = key.split('_').collect();
    ensure!(
        parts.len() == 5 && parts[0] == "eqt" && parts[1] == "trade",
        "Invalid Cboe file key"
    );
    let stream = match parts[2] {
        "BATE" => "BATE",
        "CHIX" => "CHIX",
        "CEUX" => "CEUX",
        _ => bail!("Unsupported Cboe exchange stream"),
    };
    ensure!(
        parts[3].len() == 8 && parts[3].bytes().all(|b| b.is_ascii_digit()),
        "Invalid Cboe file date"
    );
    let day = NaiveDate::parse_from_str(parts[3], "%Y%m%d").context("Invalid Cboe file date")?;
    let suffix = parts[4]
        .strip_suffix(".csv")
        .context("Invalid Cboe file suffix")?;
    let daily = suffix == "daily";
    let end = if daily {
        day.succ_opt()
            .context("Cboe file date overflow")?
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    } else {
        ensure!(
            suffix.len() == 4 && suffix.bytes().all(|b| b.is_ascii_digit()),
            "Invalid Cboe file minute"
        );
        NaiveDateTime::parse_from_str(&format!("{} {suffix}", parts[3]), "%Y%m%d %H%M")
            .context("Invalid Cboe file minute")?
            .and_utc()
            + Duration::minutes(1)
    };
    Ok(Bucket {
        stream,
        day,
        daily,
        end,
    })
}

fn validate_url(url: &Url, key: &str, bucket: &Bucket) -> anyhow::Result<()> {
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("cdn.cboe.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Unsafe Cboe download URL"
    );
    let prefix = format!(
        "/data/europe/equities/delayed_data/eqt_trade_{}/",
        bucket.stream
    );
    let rest = url
        .path()
        .strip_prefix(&prefix)
        .context("Unexpected Cboe download path")?;
    let segments: Vec<_> = rest.split('/').collect();
    ensure!(
        segments.len() == 3
            && !segments[0].is_empty()
            && segments[0].len() <= 64
            && segments[0].bytes().all(|b| b.is_ascii_alphanumeric())
            && segments[1] == (if bucket.daily { "daily" } else { "minute" })
            && segments[2] == key,
        "Unexpected Cboe download path"
    );
    Ok(())
}

fn source(
    listed: ListedFile,
    stream: &str,
    day: NaiveDate,
    daily: bool,
) -> anyhow::Result<SourceFile> {
    let coverage = bucket(&listed.filename)?;
    ensure!(
        coverage.stream == stream && coverage.day == day && coverage.daily == daily,
        "Cboe discovery returned an unexpected bucket"
    );
    let url =
        Url::parse(&listed.serve_url).map_err(|_| anyhow::anyhow!("Invalid Cboe download URL"))?;
    validate_url(&url, &listed.filename, &coverage)?;
    Ok(SourceFile {
        key: listed.filename,
        timestamp: coverage.end,
        url,
    })
}

/// Only use a daily aggregate to replace minutes when the CDN supplies its source
/// timestamp. Subtract the full publication delay, and retain that minute too.
/// The observed 2026-10-23 aggregate was stamped 16:50 but omitted the 17:00
/// publication, so treating every daily file as a final end-of-day snapshot loses data.
fn aggregate_cutoff(value: Option<&str>, day: NaiveDate) -> Option<DateTime<Utc>> {
    let stamp = timestamp(value?).ok()?;
    (stamp.date_naive() == day).then_some(stamp - Duration::minutes(15))
}

pub async fn discover(client: &Client) -> anyhow::Result<Vec<SourceFile>> {
    let now = Utc::now();
    let cutoff = now - Duration::hours(48);
    let mut files = Vec::new();
    for offset in 0..=2 {
        let day = now.date_naive() - Duration::days(offset);
        for (venue, stream) in [("bxe", "BATE"), ("cxe", "CHIX"), ("dxe", "CEUX")] {
            let response = LIMITER
                .send(client.get(API).query(&[
                    ("category", "equities"),
                    ("venue", venue),
                    ("trade_type", "trade"),
                    ("date", &day.format("%Y-%m-%d").to_string()),
                ]))
                .await?;
            http::check_status(&response)?;
            let bytes = http::limited_body(response, MAX_INDEX_BYTES).await?;
            let map: FileMap =
                serde_json::from_slice(&bytes).context("Invalid Cboe file discovery schema")?;
            ensure!(
                map.daily.len() <= 1 && map.minute.len() <= 1440,
                "Cboe discovery exceeds bucket limit"
            );
            let mut covered_until = None;
            for listed in map.daily {
                let mut file = source(listed, stream, day, true)?;
                if day < now.date_naive() && file.timestamp >= cutoff {
                    let response = LIMITER.send(client.head(file.url.clone())).await?;
                    http::check_status(&response)?;
                    covered_until = aggregate_cutoff(
                        response
                            .headers()
                            .get("x-amz-meta-file_timestamp")
                            .and_then(|v| v.to_str().ok()),
                        day,
                    );
                    if let Some(end) = covered_until {
                        // Generation time is a tighter upper coverage bound than midnight:
                        // avoid downloading an entire daily CSV whose publications have expired.
                        file.timestamp = end + Duration::minutes(15);
                    }
                    if file.timestamp >= cutoff {
                        files.push(file);
                    }
                }
            }
            for listed in map.minute {
                let file = source(listed, stream, day, false)?;
                if file.timestamp >= cutoff
                    && file.timestamp <= now + Duration::minutes(1)
                    && covered_until.is_none_or(|end| file.timestamp > end)
                {
                    files.push(file);
                }
            }
        }
    }
    // Publish current valuations first; revision precedence makes out-of-order replay safe.
    files.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.key.cmp(&b.key)));
    files.dedup_by(|a, b| a.key == b.key);
    Ok(files)
}

pub async fn download(
    client: &Client,
    file: &SourceFile,
    etag: Option<&str>,
) -> anyhow::Result<Option<Download>> {
    let coverage = bucket(&file.key)?;
    validate_url(&file.url, &file.key, &coverage)?;
    http::download(client, file, etag, &LIMITER, MAX_FILE_BYTES).await
}

// RTS publication/execution timestamps are UTC; fractional values are decimal
// seconds (the inspected CSV uses six digits). Prices are already monetary units,
// with four decimal places, NOT integer ticks or euro cents.
fn timestamp(value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .context("Invalid Cboe timestamp")?
        .and_utc())
}

fn exchange(stream: &str, publisher: &str, venue: &str) -> bool {
    match stream {
        "BATE" => {
            matches!(publisher, "BATE" | "LISX" | "XWAP")
                && matches!(venue, "BATS" | "BATD" | "BATP" | "BATF" | "LISX" | "XWAP")
        }
        "CHIX" => publisher == "CHIX" && matches!(venue, "CHIX" | "CHID" | "CHIO"),
        // BEUT also occurs in the inspected exchange file: the regulated-market
        // integrated book (NL), not an APA or OTC report.
        "CEUX" => {
            matches!(publisher, "CEUX" | "LISZ" | "VWAP")
                && matches!(
                    venue,
                    "CEUX" | "CEUD" | "BEUP" | "CEUO" | "LISZ" | "VWAP" | "BEUT"
                )
        }
        _ => false,
    }
}

pub fn parse(
    file: &SourceFile,
    bytes: &[u8],
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    let coverage = bucket(&file.key)?;
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "Cboe file exceeds size limit"
    );
    let text = std::str::from_utf8(bytes).context("Cboe file is not UTF-8")?;
    let cutoff = now - Duration::hours(48);
    let mut reports = Vec::new();
    let mut first = true;
    for (index, raw) in text.lines().enumerate() {
        ensure!(index < MAX_ROWS, "Cboe file exceeds row limit");
        ensure!(raw.len() <= MAX_LINE_BYTES, "Cboe row exceeds size limit");
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if first {
            first = false;
            if line == HEADER {
                continue;
            }
            ensure!(
                !coverage.daily,
                "Cboe daily file is missing the required schema header"
            );
            timestamp(line.split(',').next().unwrap_or_default())
                .context("Unrecognized Cboe equity CSV schema")?;
        }
        // Only Flags can contain commas or quotes in this published schema.
        // Bounded, borrowed fields avoid allocating a general CSV reader per row.
        let mut columns = line.splitn(12, ',');
        let mut row = [""; 12];
        for value in &mut row {
            *value = columns
                .next()
                .context("Cboe equity row is missing required fields")?;
        }
        ensure!(
            row[..11].iter().all(|v| !v.contains('"')),
            "Invalid Cboe equity CSV schema"
        );
        if !tracked.contains(row[1]) {
            continue;
        }
        ensure!(
            row[1].len() == 12 && row[1].bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid Cboe ISIN"
        );
        let executed = timestamp(row[0])?;
        let published = timestamp(row[8])?;
        if published < cutoff || published > now {
            continue;
        }
        let flags = row[11]
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .context("Cboe Flags must be a quoted CSV field")?;
        ensure!(
            flags.is_empty()
                || flags
                    .split(',')
                    .all(|flag| flag.len() == 4 && flag.bytes().all(|b| b.is_ascii_uppercase())),
            "Invalid Cboe trade flags"
        );
        let cancelled = flags.split(',').any(|f| f == "CANC");
        let amended = flags.split(',').any(|f| f == "AMND");
        ensure!(!(cancelled && amended), "Contradictory Cboe revision flags");
        let revision = if cancelled {
            2
        } else if amended {
            1
        } else {
            0
        };
        ensure!(
            !row[10].is_empty()
                && row[10].len() <= 52
                && row[10].bytes().all(|b| b.is_ascii_alphanumeric())
                && row[9].len() == 4
                && row[9].bytes().all(|b| b.is_ascii_uppercase()),
            "Invalid Cboe transaction identity"
        );
        let price = if row[2].is_empty() {
            Decimal::ZERO
        } else {
            Decimal::from_str(row[2]).context("Invalid Cboe price")?
        };
        let quantity = if row[5].is_empty() {
            None
        } else {
            Some(Decimal::from_str(row[5]).context("Invalid Cboe quantity")?)
        };
        let eligible = revision != 2 && row[3] == "EUR" && row[4] == "MONE"
            && price > Decimal::ZERO && quantity.is_some_and(|q| q >= Decimal::ZERO)
            && executed <= now && exchange(coverage.stream, row[9], row[6])
            && matches!(row[7], "CLOB" | "PATS" | "QDTS" | "RFQT" | "HYBR" | "OTHR")
            // Unknown conditions are not silently treated as price forming.
            && flags.split(',').all(|flag| matches!(flag, "" | "ALGO" | "AMND"));
        if revision == 0 && !eligible {
            continue;
        }
        // The original execution date, not the publication/file date, keeps late
        // amendments/cancellations on the original transaction's identity.
        let identity = format!(
            "{}|{}|{}|{}",
            Feed::Cboe.key(),
            row[9],
            executed.date_naive(),
            row[10]
        );
        reports.push(PriceReport {
            id: blake3::hash(identity.as_bytes()).to_hex().to_string(),
            isin: row[1].to_owned(),
            transaction_id: row[10].to_owned(),
            publisher: row[9].to_owned(),
            venue: row[6].to_owned(),
            executed_at: executed,
            executed_ns: executed
                .timestamp_nanos_opt()
                .context("Cboe execution time out of range")?,
            published_ns: published
                .timestamp_nanos_opt()
                .context("Cboe publication time out of range")?,
            revision,
            revision_hash: blake3::hash(line.as_bytes()).to_hex().to_string(),
            price,
            currency: row[3].to_owned(),
            eligible,
        });
    }
    ensure!(
        !first || !coverage.daily,
        "Cboe daily file is missing the required schema header"
    );
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        timestamp("2026-10-23 18:00:00").unwrap()
    }
    fn file(daily: bool) -> SourceFile {
        let key = format!(
            "eqt_trade_CEUX_20261023_{}.csv",
            if daily { "daily" } else { "1536" }
        );
        let coverage = bucket(&key).unwrap();
        SourceFile { timestamp: coverage.end,
            url: Url::parse(&format!("https://cdn.cboe.com/data/europe/equities/delayed_data/eqt_trade_CEUX/9d8b5bc6b9/{}/{key}", if daily { "daily" } else { "minute" })).unwrap(), key }
    }
    fn parsed(line: &str) -> Vec<PriceReport> {
        parse(
            &file(false),
            line.as_bytes(),
            &HashSet::from(["IT0005239360".to_owned()]),
            now(),
        )
        .unwrap()
    }
    fn row(
        price: &str,
        currency: &str,
        notation: &str,
        quantity: &str,
        venue: &str,
        flags: &str,
    ) -> String {
        format!("2026-10-23 15:17:40.937000,IT0005239360,{price},{currency},{notation},{quantity},{venue},OTHR,2026-10-23 15:36:35.990239,CEUX,00A898R00CXR,\"{flags}\"\n")
    }

    #[test]
    fn monetary_euros_are_not_cents_and_zero_quantity_is_valid() {
        let reports = parsed(&row("72.0356", "EUR", "MONE", "0", "CEUX", "ALGO"));
        assert_eq!(reports[0].price, Decimal::from_str("72.0356").unwrap());
        assert!(reports[0].eligible);
        assert_eq!(
            reports[0].executed_ns,
            timestamp("2026-10-23 15:17:40.937")
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap()
        );
        for (price, currency, notation, quantity) in [
            ("0", "EUR", "MONE", "1"),
            ("1", "USD", "MONE", "1"),
            ("1", "EUR", "PERC", "1"),
            ("1", "EUR", "MONE", "-1"),
            ("1", "EUR", "MONE", ""),
        ] {
            assert!(parsed(&row(price, currency, notation, quantity, "CEUX", "")).is_empty());
        }
    }

    #[test]
    fn exchange_conditions_and_apa_are_not_confused() {
        for venue in ["CEUX", "CEUD", "BEUP", "CEUO", "LISZ", "VWAP", "BEUT"] {
            assert!(parsed(&row("1", "EUR", "MONE", "1", venue, ""))[0].eligible);
        }
        for venue in ["XOFF", "SINT", "CAPA", "BOTC", "XXXX"] {
            assert!(parsed(&row("1", "EUR", "MONE", "1", venue, "")).is_empty());
        }
        for flag in [
            "BENC", "NPFT", "TNCP", "SDIV", "NETW", "PRIC", "NEGO", "NLIQ", "OILQ", "LRGS", "DEFF",
            "ZZZZ",
        ] {
            assert!(parsed(&row("1", "EUR", "MONE", "1", "CEUX", flag)).is_empty());
        }
    }

    #[test]
    fn real_amendment_targets_original_and_survives_ineligible_changes() {
        // Values/identity observed in the actual DXE 2026-10-23 daily file.
        let original = parsed(&row("71.5600", "EUR", "MONE", "1600", "CEUO", ""));
        let amendment = parsed(&row("72.0356", "EUR", "MONE", "1600", "CEUO", "AMND,PRIC"));
        let cancelled = parsed(&row("", "USD", "MONE", "", "CEUO", "CANC"));
        assert_eq!(original[0].id, amendment[0].id);
        assert_eq!(original[0].id, cancelled[0].id);
        assert_eq!(amendment[0].revision, 1);
        assert!(!amendment[0].eligible);
        assert_eq!(cancelled[0].revision, 2);
        assert!(!cancelled[0].eligible);
        assert_ne!(original[0].revision_hash, amendment[0].revision_hash);
        let line = row("71.5600", "EUR", "MONE", "1600", "CEUO", "");
        let daily = parse(
            &file(true),
            format!("{HEADER}\n{line}").as_bytes(),
            &HashSet::from(["IT0005239360".to_owned()]),
            now(),
        )
        .unwrap();
        assert_eq!(original[0].revision_hash, daily[0].revision_hash);
    }

    #[test]
    fn rolling_window_uses_publication_not_execution() {
        let recent = row("1", "EUR", "MONE", "1", "CEUX", "AMND").replacen(
            "2026-10-23 15:17:40.937000",
            "2026-10-19 15:17:40.937000",
            1,
        );
        assert_eq!(parsed(&recent).len(), 1);
        let old = recent.replace("2026-10-23 15:36:35.990239", "2026-10-21 17:59:59.999999");
        assert!(parsed(&old).is_empty());
        let boundary = recent.replace("2026-10-23 15:36:35.990239", "2026-10-21 18:00:00.000000");
        assert_eq!(parsed(&boundary).len(), 1);
    }

    #[test]
    fn daily_snapshot_keeps_delayed_tail_and_unknown_coverage_keeps_all_minutes() {
        let day = now().date_naive();
        let cutoff = aggregate_cutoff(Some("2026-10-23 16:50:00.091"), day).unwrap();
        assert_eq!(cutoff, timestamp("2026-10-23 16:35:00.091").unwrap());
        assert!(bucket("eqt_trade_CEUX_20261023_1635.csv").unwrap().end > cutoff);
        assert!(bucket("eqt_trade_CEUX_20261023_1700.csv").unwrap().end > cutoff);
        assert!(aggregate_cutoff(None, day).is_none());
        assert!(aggregate_cutoff(Some("2026-10-22 16:50:00"), day).is_none());
        assert_eq!(
            bucket("eqt_trade_CEUX_20261023_daily.csv").unwrap().end,
            timestamp("2026-10-24 00:00:00").unwrap()
        );
    }

    #[test]
    fn schemas_sizes_and_transport_urls_fail_closed() {
        let tracked = HashSet::from(["IT0005239360".to_owned()]);
        assert!(parse(&file(true), b"", &tracked, now()).is_err());
        assert!(parse(&file(true), b"wrong,header\n", &tracked, now()).is_err());
        assert!(parse(&file(false), b"a,b,c\n", &tracked, now()).is_err());
        assert!(parse(
            &file(false),
            "x".repeat(MAX_LINE_BYTES + 1).as_bytes(),
            &tracked,
            now()
        )
        .is_err());
        let coverage = bucket(&file(false).key).unwrap();
        assert!(validate_url(&file(false).url, &file(false).key, &coverage).is_ok());
        for bad in ["http://cdn.cboe.com/", "https://cdn.cboe.com.evil.test/", "https://cdn.cboe.com/?token=secret",
            "https://user:pass@cdn.cboe.com/", "https://cdn.cboe.com/data/europe/equities/delayed_data/eqt_trade_CEUX/x/minute/other.csv"] {
            assert!(validate_url(&Url::parse(bad).unwrap(), &file(false).key, &coverage).is_err());
        }
        assert!(bucket("eqt_trade_CAPA_20261023_1536.csv").is_err());
        assert!(bucket("eqt_trade_CEUX_20261023_2560.csv").is_err());
    }
}
