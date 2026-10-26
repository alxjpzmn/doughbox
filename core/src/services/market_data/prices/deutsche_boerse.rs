use std::{
    collections::{HashMap, HashSet},
    io::Read,
    sync::LazyLock,
};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, NaiveDateTime, Utc};
use flate2::read::MultiGzDecoder;
use reqwest::{
    header::{CACHE_CONTROL, IF_NONE_MATCH, LOCATION},
    Client, RequestBuilder, Response, StatusCode, Url,
};
use rust_decimal::Decimal;
use serde::Deserialize;

#[cfg(test)]
use super::http::http_client;
use super::{
    config::Feed,
    http::{check_status, limited_body, RequestLimiter},
    types::{Download, PriceReport, SourceFile},
};

const BASE_URL: &str = "https://mfs.deutsche-boerse.com";
const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STORAGE_ERROR_BYTES: usize = 16 * 1024;
const MAX_INFLATED_BYTES: u64 = 128 * 1024 * 1024;

// One API budget across feeds, workers and dry runs.
static REQUEST_LIMITER: LazyLock<RequestLimiter> = LazyLock::new(RequestLimiter::new);

#[derive(Deserialize)]
struct FileIndex {
    #[serde(rename = "CurrentFiles")]
    files: Vec<String>,
}

pub async fn discover(client: &Client, feed: Feed) -> anyhow::Result<Vec<SourceFile>> {
    let response = REQUEST_LIMITER
        .send(client.get(format!("{BASE_URL}/api/{}", feed.key())))
        .await?;
    check_status(&response)?;
    let index: FileIndex = serde_json::from_slice(&limited_body(response, MAX_INDEX_BYTES).await?)?;
    recent_files(index, feed, Utc::now())
}

fn recent_files(
    index: FileIndex,
    feed: Feed,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<SourceFile>> {
    let cutoff = now - chrono::Duration::hours(48);
    let mut files = Vec::new();
    for key in index.files {
        // Only immutable minute batches are supported. Do not infer daily-file schemas.
        if key.contains("daily") {
            continue;
        }
        let file = source_file(feed, key)?;
        if file.timestamp >= cutoff {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.key.cmp(&b.key)));
    files.dedup_by(|a, b| a.key == b.key);
    Ok(files)
}

fn source_file(feed: Feed, key: String) -> anyhow::Result<SourceFile> {
    let timestamp = key
        .strip_prefix(&format!("{}-", feed.key()))
        .and_then(|s| s.strip_suffix(".json.gz"))
        .context("Unexpected price file name")?;
    let timestamp = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H_%M")
        .with_context(|| format!("Unexpected price file name: {key}"))?
        .and_utc();
    // A successful parse must also round-trip, rejecting paths and URL metacharacters.
    ensure!(
        key == format!(
            "{}-{}.json.gz",
            feed.key(),
            timestamp.format("%Y-%m-%dT%H_%M")
        ),
        "Invalid price file key"
    );
    let url = Url::parse(&format!("{BASE_URL}/api/download/{key}"))?;
    Ok(SourceFile {
        key,
        timestamp,
        url,
    })
}

pub async fn download(
    client: &Client,
    file: &SourceFile,
    etag: Option<&str>,
) -> anyhow::Result<Option<Download>> {
    let mut state = DownloadState::default();
    loop {
        let request = state.request(client, file, etag);
        let response = if state.storage_url.is_some() {
            // The two-second storage signature must not wait for another API slot.
            REQUEST_LIMITER.send_now(request).await
        } else {
            REQUEST_LIMITER.send(request).await
        }
        .context(state.error_context())?;
        match state.receive(response, file).await? {
            DownloadOutcome::Continue => {}
            DownloadOutcome::Complete(download) => return Ok(download),
        }
    }
}

#[derive(Default)]
struct DownloadState {
    storage_url: Option<Url>,
    expired_url: Option<Url>,
}

enum DownloadOutcome {
    Continue,
    Complete(Option<Download>),
}

impl DownloadState {
    fn error_context(&self) -> &'static str {
        if self.storage_url.is_some() {
            "Price signed-storage download failed"
        } else {
            "Price signing API download failed"
        }
    }

    fn request(&self, client: &Client, file: &SourceFile, etag: Option<&str>) -> RequestBuilder {
        let mut request = if let Some(url) = &self.storage_url {
            client.get(url.clone())
        } else {
            // A cached redirect can already be expired before it reaches this client.
            client
                .get(file.url.clone())
                .header(CACHE_CONTROL, "no-cache")
        };
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        request
    }

    async fn receive(
        &mut self,
        response: Response,
        file: &SourceFile,
    ) -> anyhow::Result<DownloadOutcome> {
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(DownloadOutcome::Complete(None));
        }
        if self.storage_url.is_none() && response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .context("Price signing API redirect has no Location")?;
            let url =
                download_redirect(location, file).context("Price signing API redirect rejected")?;
            ensure!(
                self.expired_url.as_ref() != Some(&url),
                "Price signing API returned the expired signature after renewal"
            );
            self.storage_url = Some(url);
            return Ok(DownloadOutcome::Continue);
        }
        if self.storage_url.is_some() && response.status() == StatusCode::BAD_REQUEST {
            let body = limited_body(response, MAX_STORAGE_ERROR_BYTES)
                .await
                .context("Price signed-storage download failed (HTTP 400); reading error code")?;
            ensure!(
                is_expired_storage_token(&body),
                "Price signed-storage download failed (HTTP 400; not ExpiredToken)"
            );
            ensure!(
                self.expired_url.is_none(),
                "Price signed-storage download failed: signature expired again after renewal (HTTP 400; ExpiredToken)"
            );
            // Reacquire through the shared API limiter; never replay the rejected URL.
            self.expired_url = self.storage_url.take();
            return Ok(DownloadOutcome::Continue);
        }
        check_status(&response).context(self.error_context())?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(DownloadOutcome::Complete(Some(Download {
            bytes: limited_body(response, MAX_FILE_BYTES)
                .await
                .context(self.error_context())?,
            etag,
        })))
    }
}

fn is_expired_storage_token(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let mut text = text.trim();
    if text.starts_with("<?xml ") {
        let Some((_, rest)) = text.split_once("?>") else {
            return false;
        };
        text = rest.trim_start();
    }
    // GCS puts Code first in its Error XML. Do not match text inside a Message,
    // comment, signature diagnostic, or a differently shaped error document.
    text.strip_prefix("<Error>")
        .is_some_and(|error| error.trim_start().starts_with("<Code>ExpiredToken</Code>"))
        && text.ends_with("</Error>")
}

fn download_redirect(location: &str, file: &SourceFile) -> anyhow::Result<Url> {
    let url = Url::parse(location).context("Invalid price download redirect")?;
    let source = file
        .key
        .split('-')
        .next()
        .context("Invalid price file source")?;
    let expected_path = format!(
        "/mv-cef-prod-europe-west3-private-min-by-min-files/{source}/{}",
        file.key
    );
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("storage.googleapis.com")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.path() == expected_path,
        "Untrusted price download redirect"
    );
    Ok(url)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReport {
    message_id: String,
    instrument_identification_code: String,
    transaction_identification_code: String,
    venue_of_publication: String,
    venue_of_execution: String,
    trading_date_and_time: DateTime<Utc>,
    publication_date_and_time: DateTime<Utc>,
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    price: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    quantity: Option<Decimal>,
    #[serde(default)]
    price_currency: String,
    #[serde(default)]
    price_notation: Option<i32>,
    mmt_modification_ind: String,
    #[serde(default)]
    mmt_trading_mode: String,
    #[serde(default)]
    mmt_price_forming_ind: String,
    #[serde(default)]
    mmt_benchmark_refprc_ind: String,
    #[serde(default)]
    mmt_transaction_category: String,
    #[serde(default)]
    mmt_negot_trans_pretrd_waiv_ind: String,
    #[serde(default)]
    mmt_pub_mode_def_reason: String,
    #[serde(flatten)]
    additional_fields: HashMap<String, serde_json::Value>,
}

fn flag_is(value: &str, allowed: &[&str]) -> bool {
    value.is_empty() || allowed.contains(&value)
}

pub fn parse_gzip(
    feed: Feed,
    bytes: &[u8],
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    let mut text = String::new();
    MultiGzDecoder::new(bytes)
        .take(MAX_INFLATED_BYTES + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= MAX_INFLATED_BYTES,
        "Inflated price file exceeds size limit"
    );
    parse_ndjson(feed, &text, tracked, now)
}

pub fn parse_ndjson(
    feed: Feed,
    text: &str,
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    let mut reports = Vec::new();
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        let value: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("Invalid price JSON at line {}", index + 1))?;
        match value.get("messageId").and_then(|v| v.as_str()) {
            Some("posttrade") => {}
            // The provider mixes quote indications into some post-trade batches.
            // They have no execution identity and must never become valuations.
            Some("pretrade") => continue,
            Some(message) => bail!("Unknown price message {message:?} at line {}", index + 1),
            None => bail!("Missing or invalid price messageId at line {}", index + 1),
        }
        let isin = value
            .get("instrumentIdentificationCode")
            .and_then(|v| v.as_str())
            .context("Price message has no ISIN")?;
        if !tracked.contains(isin) {
            continue;
        }
        let raw: RawReport = serde_json::from_value(value)
            .with_context(|| format!("Invalid tracked price report at line {}", index + 1))?;
        ensure!(raw.message_id == "posttrade", "Unexpected price message");
        let revision = match raw.mmt_modification_ind.as_str() {
            "-" => 0,
            "A" => 1,
            "C" => 2,
            other => bail!("Unknown price modification indicator: {other}"),
        };
        ensure!(
            !raw.transaction_identification_code.is_empty() && !raw.venue_of_publication.is_empty(),
            "Price report has no transaction identity"
        );
        let executed_ns = raw
            .trading_date_and_time
            .timestamp_nanos_opt()
            .context("Price execution time out of range")?;
        let published_ns = raw
            .publication_date_and_time
            .timestamp_nanos_opt()
            .context("Price publication time out of range")?;
        let price = raw.price.unwrap_or(Decimal::ZERO);
        // Field mappings are documented in the provider's public view service.
        // Apply corrections even when their new price/currency/conditions are ineligible.
        let eligible = revision != 2
            && raw.price_currency == "EUR"
            && raw.price_notation == Some(1)
            && price > Decimal::ZERO
            && raw.quantity.is_some_and(|q| q >= Decimal::ZERO)
            && raw.trading_date_and_time <= now
            && raw.publication_date_and_time <= now
            && matches!(
                raw.mmt_trading_mode.as_str(),
                "2" | "3" | "4" | "O" | "K" | "I" | "U" | "P"
            )
            && !matches!(raw.venue_of_execution.as_str(), "XOFF" | "SINT")
            && flag_is(&raw.mmt_price_forming_ind, &["-", "P"])
            && flag_is(&raw.mmt_benchmark_refprc_ind, &["-"])
            && flag_is(&raw.mmt_transaction_category, &["-", "R"])
            && flag_is(&raw.mmt_negot_trans_pretrd_waiv_ind, &["-"])
            && flag_is(&raw.mmt_pub_mode_def_reason, &["-"])
            && raw.additional_fields.iter().all(|(key, value)| {
                if key == "mmtAlgoInd" {
                    return value
                        .as_str()
                        .is_some_and(|value| flag_is(value, &["-", "H"]));
                }
                if key.starts_with("mmt") {
                    // Do not ignore an unfamiliar, populated special-condition flag.
                    return value.as_str().is_some_and(|value| flag_is(value, &["-"]));
                }
                if key == "tradeIndicator" {
                    return value
                        .as_str()
                        .is_some_and(|value| !matches!(value, "a" | "k"));
                }
                true
            });
        let identity = format!(
            "{}|{}|{}|{}",
            feed.key(),
            raw.venue_of_publication,
            raw.trading_date_and_time.date_naive(),
            raw.transaction_identification_code
        );
        reports.push(PriceReport {
            id: blake3::hash(identity.as_bytes()).to_hex().to_string(),
            isin: raw.instrument_identification_code,
            transaction_id: raw.transaction_identification_code,
            publisher: raw.venue_of_publication,
            venue: raw.venue_of_execution,
            executed_at: raw.trading_date_and_time,
            executed_ns,
            published_ns,
            revision,
            revision_hash: blake3::hash(line.as_bytes()).to_hex().to_string(),
            price,
            currency: raw.price_currency,
            eligible,
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use rust_decimal_macros::dec;
    use std::io::Write;
    use tokio::sync::Mutex;

    const DOWNLOAD_TEST_KEY: &str = "DFRA-posttrade-2026-10-15T14_30.json.gz";
    const EXPIRED_TOKEN_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>ExpiredToken</Code><Message>secret credential canonical signature</Message></Error>";
    type DownloadReply = (String, StatusCode, Vec<(&'static str, String)>, Vec<u8>);

    fn signing_url() -> String {
        format!("{BASE_URL}/api/download/{DOWNLOAD_TEST_KEY}")
    }

    fn storage_url(signature: &str) -> String {
        format!("https://storage.googleapis.com/mv-cef-prod-europe-west3-private-min-by-min-files/DFRA/{DOWNLOAD_TEST_KEY}?X-Goog-Signature={signature}")
    }

    fn signing_redirect(signature: &str) -> DownloadReply {
        (
            signing_url(),
            StatusCode::MOVED_PERMANENTLY,
            vec![("location", storage_url(signature))],
            Vec::new(),
        )
    }

    fn storage_reply(signature: &str, status: StatusCode, body: &str) -> DownloadReply {
        (
            storage_url(signature),
            status,
            vec![("content-type", "application/xml".into())],
            body.as_bytes().to_vec(),
        )
    }

    // Exercise the production request builder and protocol state with real HTTP
    // responses. Only the test transport changes origins; redirect validation is
    // unchanged and every requested signed URL must match the script exactly.
    async fn scripted_download(
        replies: Vec<DownloadReply>,
        etag: Option<&str>,
    ) -> anyhow::Result<Option<Download>> {
        use axum::{extract::State, http::HeaderMap, response::IntoResponse, Router};
        use std::{collections::VecDeque, sync::Arc};

        let expected_urls: Vec<_> = replies.iter().map(|reply| reply.0.clone()).collect();
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let conditional = etag.map(str::to_owned);
        let app = Router::new()
            .fallback(
                move |State(replies): State<Arc<Mutex<VecDeque<DownloadReply>>>>,
                      uri: axum::http::Uri,
                      headers: HeaderMap| {
                    let conditional = conditional.clone();
                    async move {
                        let (expected_url, status, response_headers, body) = replies
                            .lock()
                            .await
                            .pop_front()
                            .expect("Unexpected request");
                        let expected_url = Url::parse(&expected_url).unwrap();
                        assert_eq!(uri.path(), expected_url.path());
                        assert_eq!(uri.query(), expected_url.query());
                        assert_eq!(
                            headers
                                .get(IF_NONE_MATCH)
                                .map(|value| value.to_str().unwrap()),
                            conditional.as_deref()
                        );
                        if expected_url.host_str() == Some("mfs.deutsche-boerse.com") {
                            assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-cache");
                        }
                        let mut response = (status, body).into_response();
                        for (name, value) in response_headers {
                            response.headers_mut().insert(
                                axum::http::HeaderName::from_static(name),
                                value.parse().unwrap(),
                            );
                        }
                        response
                    }
                },
            )
            .with_state(replies.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = http_client().unwrap();
        let file = source_file(Feed::Frankfurt, DOWNLOAD_TEST_KEY.into()).unwrap();
        let mut state = DownloadState::default();
        let mut result = None;
        for expected_url in expected_urls {
            let mut request = state.request(&client, &file, etag).build().unwrap();
            assert_eq!(request.url().as_str(), expected_url);
            request.url_mut().set_scheme("http").unwrap();
            request.url_mut().set_host(Some("127.0.0.1")).unwrap();
            request.url_mut().set_port(Some(address.port())).unwrap();
            let response = client.execute(request).await.unwrap();
            match state.receive(response, &file).await {
                Ok(DownloadOutcome::Continue) => {}
                Ok(DownloadOutcome::Complete(download)) => {
                    result = Some(Ok(download));
                    break;
                }
                Err(error) => {
                    result = Some(Err(error));
                    break;
                }
            }
        }
        server.abort();
        assert!(
            replies.lock().await.is_empty(),
            "Unexpected early termination"
        );
        result.expect("Protocol requested an unplanned additional request")
    }

    fn assert_safe_download_error(error: anyhow::Error) {
        let message = format!("{error:#}");
        for secret in ["secret", "https://", "X-Goog-", "<Error>", "canonical"] {
            assert!(!message.contains(secret), "{message}");
        }
    }

    #[tokio::test]
    async fn expired_signed_download_reacquires_once_and_preserves_conditionals_and_etag() {
        let mut success = storage_reply("fresh", StatusCode::OK, "download bytes");
        success.2.push(("etag", "\"new-etag\"".into()));
        let download = scripted_download(
            vec![
                signing_redirect("expired"),
                storage_reply("expired", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
                signing_redirect("fresh"),
                success,
            ],
            Some("\"old-etag\""),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(download.bytes, b"download bytes");
        assert_eq!(download.etag.as_deref(), Some("\"new-etag\""));
    }

    #[tokio::test]
    async fn second_signed_download_expiry_terminates_without_a_third_signature() {
        let error = scripted_download(
            vec![
                signing_redirect("expired"),
                storage_reply("expired", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
                signing_redirect("fresh"),
                storage_reply("fresh", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
            ],
            None,
        )
        .await
        .err()
        .unwrap();
        assert_safe_download_error(error);
    }

    #[tokio::test]
    async fn signing_api_cannot_replay_the_rejected_signature() {
        let error = scripted_download(
            vec![
                signing_redirect("secret-expired"),
                storage_reply("secret-expired", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
                signing_redirect("secret-expired"),
            ],
            None,
        )
        .await
        .err()
        .unwrap();
        assert_safe_download_error(error);
    }

    #[tokio::test]
    async fn unrelated_storage_errors_do_not_renew_signatures() {
        for (status, body) in [
            (
                StatusCode::BAD_REQUEST,
                "<Error><Code>SignatureDoesNotMatch</Code><Message>secret</Message></Error>",
            ),
            (StatusCode::FORBIDDEN, EXPIRED_TOKEN_XML),
            (StatusCode::NOT_FOUND, EXPIRED_TOKEN_XML),
            (
                StatusCode::BAD_REQUEST,
                "<Error><Message><Code>ExpiredToken</Code></Message></Error>",
            ),
            (
                StatusCode::BAD_REQUEST,
                "<Error><!-- <Code>ExpiredToken</Code> --></Error>",
            ),
        ] {
            let error = scripted_download(
                vec![
                    signing_redirect("secret"),
                    storage_reply("secret", status, body),
                ],
                None,
            )
            .await
            .err()
            .unwrap();
            assert_safe_download_error(error);
        }
    }

    #[tokio::test]
    async fn signing_api_errors_and_oversized_storage_xml_do_not_renew() {
        let error = scripted_download(
            vec![(
                signing_url(),
                StatusCode::BAD_REQUEST,
                Vec::new(),
                EXPIRED_TOKEN_XML.as_bytes().to_vec(),
            )],
            None,
        )
        .await
        .err()
        .unwrap();
        assert_safe_download_error(error);
        let oversized = format!(
            "<Error><Code>ExpiredToken</Code><Message>{}</Message></Error>",
            "x".repeat(MAX_STORAGE_ERROR_BYTES)
        );
        let error = scripted_download(
            vec![
                signing_redirect("expired"),
                storage_reply("expired", StatusCode::BAD_REQUEST, &oversized),
            ],
            None,
        )
        .await
        .err()
        .unwrap();
        assert_safe_download_error(error);
    }

    #[tokio::test]
    async fn conditional_downloads_accept_not_modified_at_either_stage_including_renewal() {
        let not_modified_api = || {
            (
                signing_url(),
                StatusCode::NOT_MODIFIED,
                Vec::new(),
                Vec::new(),
            )
        };
        for replies in [
            vec![not_modified_api()],
            vec![
                signing_redirect("fresh"),
                storage_reply("fresh", StatusCode::NOT_MODIFIED, ""),
            ],
            vec![
                signing_redirect("expired"),
                storage_reply("expired", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
                not_modified_api(),
            ],
            vec![
                signing_redirect("expired"),
                storage_reply("expired", StatusCode::BAD_REQUEST, EXPIRED_TOKEN_XML),
                signing_redirect("fresh"),
                storage_reply("fresh", StatusCode::NOT_MODIFIED, ""),
            ],
        ] {
            assert!(scripted_download(replies, Some("\"unchanged\""))
                .await
                .unwrap()
                .is_none());
        }
    }

    fn sample() -> serde_json::Value {
        serde_json::from_str(r#"{"messageId":"posttrade","instrumentIdentificationCode":"DE0007164600","transactionIdentificationCode":"test-1","venueOfPublication":"XCEF","venueOfExecution":"XETA","tradingDateAndTime":"2026-10-15T14:30:00.137618751Z","publicationDateAndTime":"2026-10-15T14:30:00.147705000Z","price":201.600000000000000001,"quantity":91,"priceCurrency":"EUR","priceNotation":1,"mmtModificationInd":"-","mmtTradingMode":"2"}"#).unwrap()
    }

    fn parse(value: serde_json::Value) -> PriceReport {
        parse_ndjson(
            Feed::Xetra,
            &value.to_string(),
            &HashSet::from(["DE0007164600".into()]),
            "2026-10-15T16:00:00Z".parse().unwrap(),
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn parses_gzip_ndjson_without_losing_decimal_or_timestamp_precision() {
        let text = format!("{}\n{}\n", sample(), sample());
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(text.as_bytes()).unwrap();
        let reports = parse_gzip(
            Feed::Xetra,
            &gzip.finish().unwrap(),
            &HashSet::from(["DE0007164600".into()]),
            "2026-10-15T16:00:00Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].price, dec!(201.600000000000000001));
        assert_eq!(reports[0].executed_at.timestamp_subsec_nanos(), 137618751);
        assert!(reports[0].eligible);
    }

    #[test]
    fn frankfurt_prices_preserve_provenance_including_zero_quantity_reports() {
        // Shape and flags from a live DFRA minute batch; quantity zero is also common.
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{"messageId":"posttrade","instrumentIdentificationCode":"JP3122400009","transactionIdentificationCode":"3000000000000000044240179216100784982633400000004976","venueOfPublication":"XCEF","venueOfExecution":"FRAB","tradingDateAndTime":"2026-10-16T14:30:07.849826334Z","publicationDateAndTime":"2026-10-16T14:30:07.860419000Z","price":190.54,"quantity":1.0,"priceCurrency":"EUR","priceNotation":1,"mmtModificationInd":"-","mmtTradingMode":"U","mmtAlgoInd":"-","mmtPubModeDefReason":"-","notionalCurrency":"EUR","tradingSystem":"9"}"#,
        )
        .unwrap();
        let tracked = HashSet::from(["JP3122400009".into()]);
        let now = "2026-10-16T16:00:00Z".parse().unwrap();
        let execution = parse_ndjson(Feed::Frankfurt, &value.to_string(), &tracked, now)
            .unwrap()
            .remove(0);
        assert!(execution.eligible);
        assert_eq!(execution.price, dec!(190.54));
        assert_eq!(execution.venue, "FRAB");
        assert_eq!(execution.publisher, "XCEF");
        assert_eq!(execution.executed_at.timestamp_subsec_nanos(), 849826334);
        value["quantity"] = serde_json::json!(0);
        let indication = parse_ndjson(Feed::Frankfurt, &value.to_string(), &tracked, now)
            .unwrap()
            .remove(0);
        assert!(indication.eligible);
    }

    #[test]
    fn unsuitable_prices_are_not_candidates() {
        for (key, value) in [
            ("priceCurrency", serde_json::json!("USD")),
            ("priceNotation", serde_json::json!(2)),
            ("quantity", serde_json::json!(-1)),
            ("quantity", serde_json::Value::Null),
            ("price", serde_json::json!(-1)),
            ("mmtPriceFormingInd", serde_json::json!("T")),
            ("mmtPriceFormingInd", serde_json::json!("J")),
            ("mmtBenchmarkRefprcInd", serde_json::json!("B")),
            ("mmtTradingMode", serde_json::json!("6")),
            ("mmtTransactionCategory", serde_json::json!("Z")),
        ] {
            let mut value_with_flag = sample();
            value_with_flag[key] = value;
            assert!(!parse(value_with_flag).eligible, "{key}");
        }
    }

    #[test]
    fn cancellation_retains_identity_even_without_a_price() {
        let original = parse(sample());
        let mut cancellation = sample();
        cancellation["mmtModificationInd"] = serde_json::json!("C");
        cancellation["price"] = serde_json::Value::Null;
        let cancelled = parse(cancellation);
        assert_eq!(cancelled.id, original.id);
        assert_eq!(cancelled.revision, 2);
        assert!(!cancelled.eligible);
    }

    #[test]
    fn mixed_pretrade_indications_do_not_hide_executions_or_cancellations() {
        let mut original = sample();
        original["venueOfExecution"] = serde_json::json!("XGAT");
        let mut indication = original.clone();
        indication["messageId"] = serde_json::json!("pretrade");
        indication["price"] = serde_json::json!(99999);
        indication
            .as_object_mut()
            .unwrap()
            .remove("transactionIdentificationCode");
        let mut cancellation = original.clone();
        cancellation["mmtModificationInd"] = serde_json::json!("C");
        cancellation["price"] = serde_json::Value::Null;
        let text = format!("{original}\n{indication}\n{cancellation}\n");
        let reports = parse_ndjson(
            Feed::Tradegate,
            &text,
            &HashSet::from(["DE0007164600".into()]),
            "2026-10-15T16:00:00Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(reports.len(), 2);
        assert!(reports[0].eligible);
        assert_eq!(reports[0].price, dec!(201.600000000000000001));
        assert_eq!(reports[1].id, reports[0].id);
        assert_eq!(reports[1].revision, 2);
        assert!(!reports[1].eligible);
    }

    #[test]
    fn unknown_schema_and_modifications_fail_closed() {
        let tracked = HashSet::from(["DE0007164600".into()]);
        let now = Utc::now();
        assert!(parse_ndjson(Feed::Xetra, "{}", &tracked, now).is_err());
        assert!(parse_ndjson(
            Feed::Xetra,
            r#"{"messageId":"posttrade-v2"}"#,
            &tracked,
            now
        )
        .is_err());
        let mut value = sample();
        value["mmtModificationInd"] = serde_json::json!("unexpected");
        assert!(parse_ndjson(Feed::Xetra, &value.to_string(), &tracked, now).is_err());
    }

    #[test]
    fn source_files_cannot_escape_the_fixed_download_origin() {
        assert!(source_file(
            Feed::Frankfurt,
            "DFRA-posttrade-2026-10-15T14_30.json.gz".into()
        )
        .is_ok());
        for key in [
            "../other.json.gz",
            "https://example.com/file.json.gz",
            "DGAT-posttrade-2026-10-15T14_30.json.gz",
        ] {
            assert!(source_file(Feed::Frankfurt, key.into()).is_err());
        }
    }

    #[test]
    fn discovery_limits_all_feeds_to_48_hours_even_over_a_weekend() {
        let now = "2026-10-25T12:30:00Z".parse().unwrap();
        for feed in [Feed::Xetra, Feed::Tradegate, Feed::Frankfurt] {
            let key = |timestamp| format!("{}-{timestamp}.json.gz", feed.key());
            let files = recent_files(
                FileIndex {
                    files: vec![
                        key("2026-10-23T12_29"),
                        key("2026-10-23T12_30"),
                        key("2026-10-23T21_00"),
                    ],
                },
                feed,
                now,
            )
            .unwrap();
            assert_eq!(
                files.into_iter().map(|file| file.key).collect::<Vec<_>>(),
                [key("2026-10-23T12_30"), key("2026-10-23T21_00")]
            );
            let old_only = recent_files(
                FileIndex {
                    files: vec![key("2026-10-23T12_29")],
                },
                feed,
                now,
            )
            .unwrap();
            assert!(old_only.is_empty());
        }
    }

    #[test]
    fn algorithmic_trades_are_valid_but_unfamiliar_special_conditions_are_not() {
        let mut value = sample();
        value["mmtAlgoInd"] = serde_json::json!("H");
        assert!(parse(value.clone()).eligible);
        value["mmtSpecialDividendInd"] = serde_json::json!("S");
        assert!(!parse(value).eligible);
    }

    #[test]
    fn signed_download_redirects_are_restricted_to_the_expected_bucket_and_file() {
        let file = source_file(
            Feed::Frankfurt,
            "DFRA-posttrade-2026-10-15T14_30.json.gz".into(),
        )
        .unwrap();
        let path = format!(
            "/mv-cef-prod-europe-west3-private-min-by-min-files/DFRA/{}",
            file.key
        );
        assert!(download_redirect(
            &format!("https://storage.googleapis.com{path}?signature=example"),
            &file
        )
        .is_ok());
        for location in [
            format!("http://storage.googleapis.com{path}"),
            format!("https://storage.googleapis.com.evil.example{path}"),
            format!("https://storage.googleapis.com:444{path}"),
            format!("https://user@storage.googleapis.com{path}"),
            "https://storage.googleapis.com/other-bucket/file".into(),
            format!("https://storage.googleapis.com/mv-cef-prod-europe-west3-private-min-by-min-files/DETR/{}", file.key),
        ] {
            assert!(download_redirect(&location, &file).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "Optional live provider smoke test; requires internet, never writes to the database"]
    async fn pricing_live_provider_smoke() {
        let client = http_client().unwrap();
        for feed in [Feed::Xetra, Feed::Tradegate, Feed::Frankfurt] {
            let cutoff = Utc::now() - chrono::Duration::hours(48);
            let files = discover(&client, feed).await.unwrap();
            assert!(files.iter().all(|file| file.timestamp >= cutoff));
            println!(
                "{}: {} batches in the last 48 hours",
                feed.key(),
                files.len()
            );
            let Some(latest) = files.last() else {
                continue;
            };
            let file = files
                .iter()
                .rev()
                .find(|file| file.key.contains("T14_30"))
                .unwrap_or(latest);
            let file = download(&client, file, None).await.unwrap().unwrap();
            let tracked = HashSet::from(["DE0007164600".into()]);
            let reports = parse_gzip(feed, &file.bytes, &tracked, Utc::now()).unwrap();
            assert!(reports.iter().all(|report| report.isin == "DE0007164600"));
            if !reports.is_empty() {
                assert!(reports.iter().any(|report| report.eligible));
            }
            println!(
                "{}: validated live gzip NDJSON: {} bytes, {} tracked reports",
                feed.key(),
                file.bytes.len(),
                reports.len()
            );
        }
    }
}
