use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use anyhow::{anyhow, bail, ensure, Result};
use reqwest::{Client, Url};

use crate::services::shared::http::{limited_body, ResponseTooLarge};

const MAX_REPORT_BYTES: usize = 25 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(crate) struct ReportTooLarge;

impl std::fmt::Display for ReportTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Trading 212 history report exceeds the 25 MiB size limit")
    }
}

impl std::error::Error for ReportTooLarge {}

#[cfg(test)]
static TEST_DOWNLOAD_URL: std::sync::Mutex<Option<Url>> = std::sync::Mutex::new(None);

/// Serialized fixtures may permit their own exact loopback origin only. This is
/// deliberately independent of the broker client and does not exist in production.
#[cfg(test)]
pub(crate) fn set_test_download_url(value: Option<String>) -> Result<()> {
    let url = value.as_deref().map(parse_url).transpose()?;
    if let Some(url) = &url {
        ensure!(
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port != 0)
                && url.path() == "/"
                && url.query().is_none(),
            "Test report download URL must be an explicit loopback HTTP origin"
        );
    }
    *TEST_DOWNLOAD_URL.lock().map_err(|_| anyhow!("Test download configuration unavailable"))? = url;
    Ok(())
}

fn parse_url(value: &str) -> Result<Url> {
    // Inspect the original authority too: URL normalization can erase empty userinfo.
    let authority = value.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .ok_or_else(|| anyhow!("Report download requires an absolute HTTPS URL"))?;
    ensure!(!authority.contains('@'), "Report download URL must not contain userinfo");
    let url = Url::parse(value).map_err(|_| anyhow!("Report download URL is invalid"))?;
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Report download URL must have a host and no userinfo or fragment"
    );
    Ok(url)
}

fn validate_production_url(url: &Url) -> Result<()> {
    ensure!(
        url.scheme() == "https" && url.port_or_known_default() == Some(443),
        "Report download requires HTTPS on port 443"
    );
    Ok(())
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    // Fail closed for special-purpose, documentation, benchmarking, multicast
    // and reserved ranges, not merely RFC1918 and loopback.
    !(a == 0 || a == 10 || a == 127 || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

fn public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // Only ordinary global unicast is eligible. This also rejects mapped IPv4,
    // NAT64, ULA, link-local, unspecified, multicast and other reserved space.
    (segments[0] & 0xe000) == 0x2000
        && !(segments[0] == 0x2001 && segments[1] < 0x0200)
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && segments[0] != 0x2002 // 6to4 embeds a second, unvalidated destination.
        && !(segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
}

fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => public_ipv6(ip),
    }
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
    ensure!(
        !addresses.is_empty() && addresses.iter().all(|address| public_address(address.ip())),
        "Report download destination is not a public address"
    );
    Ok(())
}

/// A fresh transport has no broker credentials, cookies, ambient proxies,
/// redirects or protocol retries. Every resolved address is checked before the
/// same addresses are pinned into reqwest's resolver, avoiding DNS rebinding.
pub(crate) async fn download_report(value: &str) -> Result<Vec<u8>> {
    tokio::time::timeout(TOTAL_TIMEOUT, download(value))
        .await
        .map_err(|_| anyhow!("Report download timed out"))?
}

async fn download(value: &str) -> Result<Vec<u8>> {
    let url = parse_url(value)?;
    #[cfg(test)]
    let fixture = TEST_DOWNLOAD_URL.lock()
        .map_err(|_| anyhow!("Test download configuration unavailable"))?
        .as_ref().is_some_and(|allowed| allowed.origin() == url.origin());
    #[cfg(not(test))]
    let fixture = false;

    if !fixture {
        validate_production_url(&url)?;
    }
    let hostname = url.host_str().ok_or_else(|| anyhow!("Report download host is missing"))?;
    let port = url.port_or_known_default().ok_or_else(|| anyhow!("Report download port is invalid"))?;
    let addresses = if let Ok(ip) = hostname.trim_matches(['[', ']']).parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((hostname, port)))
            .await
            .map_err(|_| anyhow!("Report download DNS lookup timed out"))?
            .map_err(|_| anyhow!("Report download DNS lookup failed"))?
            .collect::<Vec<_>>()
    };
    if !fixture {
        validate_addresses(&addresses)?;
    }
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .resolve_to_addrs(hostname, &addresses)
        .build()
        .map_err(|_| anyhow!("Report download transport could not be created"))?;
    let response = client.get(url).send().await
        .map_err(|_| anyhow!("Report download request failed"))?;
    if response.status().is_redirection() {
        bail!("Report download redirects are not permitted");
    }
    ensure!(response.status().is_success(), "Report download failed (HTTP {})", response.status().as_u16());
    limited_body(response, MAX_REPORT_BYTES).await.map_err(|error| {
        if error.is::<ResponseTooLarge>() {
            ReportTooLarge.into()
        } else {
            // Drop the entire transport error chain; signed URLs must never escape.
            anyhow!("Report download body could not be read")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, Bytes},
        http::{header, HeaderMap, Response, StatusCode},
        routing::get,
        Router,
    };
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

    #[test]
    fn url_and_public_destination_validation_fail_closed() {
        for value in [
            "not a URL", "//example.com/report", "https://user:secret@example.com/report",
            "https://@example.com/report", "https://example.com/report#fragment",
        ] {
            assert!(parse_url(value).is_err());
        }
        for value in ["http://example.com/report", "https://example.com:444/report", "ftp://example.com/report"] {
            assert!(validate_production_url(&parse_url(value).unwrap()).is_err());
        }
        validate_production_url(&parse_url("https://example.com:443/report?signature=secret").unwrap()).unwrap();
        for value in [
            "0.0.0.0", "0.1.2.3", "10.1.2.3", "100.64.0.1", "100.127.255.255",
            "127.0.0.1", "169.254.169.254", "172.16.0.1", "172.31.255.255",
            "192.0.0.1", "192.0.2.1", "192.88.99.1", "192.168.0.1", "198.18.0.1",
            "198.19.255.255", "198.51.100.1", "203.0.113.1", "224.0.0.1", "255.255.255.255",
            "::", "::1", "::ffff:127.0.0.1", "::ffff:8.8.8.8", "64:ff9b::7f00:1",
            "100::1", "2001::1", "2001:2::1", "2001:20::1", "2001:db8::1", "2002:7f00:1::",
            "3fff::1", "fc00::1", "fe80::1", "ff02::1", "4000::1",
        ] {
            assert!(!public_address(value.parse().unwrap()), "{value}");
        }
        for value in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111", "2001:4860:4860::8888"] {
            assert!(public_address(value.parse().unwrap()), "{value}");
        }
        assert!(validate_addresses(&[]).is_err());
        assert!(validate_addresses(&[
            "8.8.8.8:443".parse().unwrap(), "127.0.0.1:443".parse().unwrap(),
        ]).is_err());
    }

    #[tokio::test]
    async fn actual_download_has_no_credentials_redirects_or_unbounded_bodies() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) { set_test_download_url(None).unwrap(); }
        }
        let _reset = Reset;
        for value in [
            "http://localhost:12345/", "http://127.0.0.2:12345/", "https://127.0.0.1:12345/",
            "http://127.0.0.1/", "http://127.0.0.1:0/", "http://127.0.0.1:12345/report",
            "http://user@127.0.0.1:12345/", "http://127.0.0.1:12345/?secret=x", "http://127.0.0.1:12345/#x",
        ] {
            assert!(set_test_download_url(Some(value.into())).is_err());
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = Router::new()
            .route("/report", get(move |headers: HeaderMap| {
                let observed = observed.clone();
                async move {
                    assert!(!headers.contains_key(header::AUTHORIZATION));
                    assert!(!headers.contains_key(header::COOKIE));
                    assert!(!headers.contains_key(header::PROXY_AUTHORIZATION));
                    observed.fetch_add(1, Ordering::SeqCst);
                    ([(header::SET_COOKIE, "broker_session=must-not-be-replayed")], "Time,Action\n")
                }
            }))
            .route("/redirect", get(|| async {
                (StatusCode::FOUND, [(header::LOCATION, "/report?signature=secret")])
            }))
            .route("/large", get(|| async {
                Response::builder().header(header::CONTENT_LENGTH, MAX_REPORT_BYTES + 1)
                    .body(Body::from(vec![0_u8; MAX_REPORT_BYTES + 1])).unwrap()
            }))
            .route("/chunked", get(|| async {
                let stream = futures::stream::iter((0..26).map(|_| {
                    Ok::<_, std::io::Error>(Bytes::from_static(&[b'x'; 1024 * 1024]))
                }));
                Response::new(Body::from_stream(stream))
            }))
            .route("/broken", get(|| async {
                let stream = futures::stream::iter([
                    Ok(Bytes::from_static(b"Time,Action\n")),
                    Err(std::io::Error::other("fixture body failure")),
                ]);
                Response::new(Body::from_stream(stream))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Production rules apply even in tests until the exact fixture is declared.
        assert!(download_report(&format!("{origin}/report")).await.is_err());
        assert!(download_report("https://127.0.0.1/report").await.is_err());
        assert!(download_report("https://[::1]/report").await.is_err());
        set_test_download_url(Some(format!("{origin}/"))).unwrap();
        for _ in 0..2 {
            assert_eq!(download_report(&format!("{origin}/report?signature=secret")).await.unwrap(), b"Time,Action\n");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let error = download_report(&format!("{origin}/redirect?signature=secret")).await.unwrap_err();
        assert!(error.to_string().contains("redirect"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for path in ["large", "chunked"] {
            let error = download_report(&format!("{origin}/{path}?signature=secret")).await.unwrap_err();
            assert!(error.is::<ReportTooLarge>());
            assert!(!format!("{error:#}").contains("secret"));
        }
        let error = download_report(&format!("{origin}/broken?signature=secret")).await.unwrap_err();
        assert!(!format!("{error:#}").contains("secret"));
        let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_address = other_listener.local_addr().unwrap();
        assert!(download_report(&format!("http://{other_address}/report")).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        set_test_download_url(None).unwrap();
        assert!(download_report(&format!("{origin}/report")).await.is_err());
        server.abort();
    }
}
