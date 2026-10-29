use std::time::Duration;

use anyhow::ensure;
use chrono::{DateTime, Utc};
use reqwest::{
    header::{ETAG, IF_NONE_MATCH},
    Client, RequestBuilder, Response, StatusCode,
};
use tokio::{
    sync::Mutex,
    time::{sleep_until, Instant},
};

use super::types::{Download, SourceFile};

const REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const MAX_REQUEST_INTERVAL: Duration = Duration::from_secs(60);

pub(super) struct RequestLimiter {
    state: Mutex<RequestSchedule>,
}

struct RequestSchedule {
    next_request: Instant,
    interval: Duration,
}

impl RequestLimiter {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RequestSchedule {
                next_request: Instant::now(),
                interval: REQUEST_INTERVAL,
            }),
        }
    }

    async fn wait(&self) {
        loop {
            let mut state = self.state.lock().await;
            let now = Instant::now();
            if now >= state.next_request {
                // Base each slot on the actual start, not a backlog of reserved slots.
                state.next_request = now + state.interval;
                return;
            }
            let deadline = state.next_request;
            drop(state);
            sleep_until(deadline).await;
            // A 429 from an in-flight request may have extended the deadline.
        }
    }

    async fn defer(&self, duration: Duration) {
        let mut state = self.state.lock().await;
        // A cooldown alone would resume at the same rejected pace. Keep the slower
        // spacing for this process, including requests queued by other feed workers.
        state.interval = state.interval.saturating_mul(2).min(MAX_REQUEST_INTERVAL);
        state.next_request = state
            .next_request
            .max(Instant::now() + duration.max(state.interval));
    }

    pub(super) async fn send(&self, request: RequestBuilder) -> anyhow::Result<Response> {
        self.wait().await;
        self.send_now(request).await
    }

    pub(super) async fn send_now(&self, request: RequestBuilder) -> anyhow::Result<Response> {
        let response = request.send().await.map_err(|error| error.without_url())?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let error = rate_limited(&response);
            self.defer(Duration::from_secs(error.retry_after_seconds as u64))
                .await;
            return Err(error.into());
        }
        Ok(response)
    }
}

#[derive(Debug)]
pub struct RateLimited {
    pub retry_after_seconds: i32,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Price provider rate limited the request; retry after {} seconds",
            self.retry_after_seconds
        )
    }
}

impl std::error::Error for RateLimited {}
pub fn http_client() -> anyhow::Result<Client> {
    // Redirects are validated explicitly by the download adapter, never followed blindly.
    Ok(Client::builder()
        .user_agent(concat!("Doughbox/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?)
}

pub(super) async fn limited_body(mut response: Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let length = response.content_length();
    ensure!(
        length.is_none_or(|n| n <= limit as u64),
        "Price response exceeds size limit"
    );
    let mut bytes = Vec::with_capacity(length.unwrap_or(0) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())?
    {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Price response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn rate_limited(response: &Response) -> RateLimited {
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok());
    let seconds = retry_after
        .and_then(|value| {
            value.parse::<i32>().ok().or_else(|| {
                DateTime::parse_from_rfc2822(value).ok().map(|date| {
                    (date.with_timezone(&Utc) - Utc::now())
                        .num_seconds()
                        .clamp(1, 86400) as i32
                })
            })
        })
        .unwrap_or(60)
        .clamp(1, 86400);
    RateLimited {
        retry_after_seconds: seconds,
    }
}

pub(super) fn check_status(response: &Response) -> anyhow::Result<()> {
    response
        .error_for_status_ref()
        .map_err(|error| error.without_url())?;
    ensure!(
        !response.status().is_redirection(),
        "Unexpected price provider redirect"
    );
    Ok(())
}

pub(super) async fn download(
    client: &Client,
    file: &SourceFile,
    etag: Option<&str>,
    limiter: &RequestLimiter,
    max_bytes: usize,
) -> anyhow::Result<Option<Download>> {
    // Bulk CSVs can exceed 250 MiB; signing/index requests keep the shorter client timeout.
    let mut request = client
        .get(file.url.clone())
        .timeout(Duration::from_secs(300));
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = limiter.send(request).await?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    check_status(&response)?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = limited_body(response, max_bytes).await?;
    Ok(Some(Download { bytes, etag }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn concurrent_requests_remain_spaced_after_idle_time() {
        let limiter = RequestLimiter::new();
        limiter.wait().await;
        tokio::time::advance(Duration::from_secs(60)).await;
        let idle_end = Instant::now();
        let started = || async {
            limiter.wait().await;
            Instant::now()
        };
        let (a, b, c) = tokio::join!(started(), started(), started());
        let mut starts = [a, b, c];
        starts.sort_unstable();
        assert_eq!(starts[0], idle_end);
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] >= REQUEST_INTERVAL);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cooldown_extends_pending_requests_and_cannot_be_shortened() {
        let limiter = RequestLimiter::new();
        limiter.wait().await;
        let pending = limiter.wait();
        tokio::pin!(pending);
        assert!(futures::poll!(&mut pending).is_pending());
        let resume_at = Instant::now() + Duration::from_secs(60);
        limiter.defer(Duration::from_secs(60)).await;
        limiter.defer(Duration::from_secs(2)).await;
        tokio::time::advance(REQUEST_INTERVAL).await;
        assert!(futures::poll!(&mut pending).is_pending());
        pending.await;
        assert_eq!(Instant::now(), resume_at);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_requests_do_not_reserve_future_slots() {
        let limiter = RequestLimiter::new();
        let first = Instant::now();
        limiter.wait().await;
        {
            let cancelled = limiter.wait();
            tokio::pin!(cancelled);
            assert!(futures::poll!(&mut cancelled).is_pending());
        }
        limiter.wait().await;
        assert_eq!(Instant::now() - first, REQUEST_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_rate_limits_slow_requests_without_unbounded_spacing() {
        let limiter = RequestLimiter::new();
        limiter.wait().await;
        let mut previous_spacing = REQUEST_INTERVAL;
        for _ in 0..8 {
            limiter.defer(Duration::from_secs(60)).await;
            limiter.wait().await;
            let resumed_at = Instant::now();
            limiter.wait().await;
            let spacing = Instant::now() - resumed_at;
            assert!(spacing >= previous_spacing);
            assert!(spacing <= MAX_REQUEST_INTERVAL);
            if previous_spacing < MAX_REQUEST_INTERVAL {
                assert!(spacing > previous_spacing);
            }
            previous_spacing = spacing;
        }
        assert_eq!(previous_spacing, MAX_REQUEST_INTERVAL);
    }
    #[tokio::test]
    async fn http_limits_redirects_and_retry_after_are_enforced() {
        use axum::{routing::get, Router};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/large", get(|| async { "too many bytes" }))
            .route(
                "/redirect",
                get(|| async { (StatusCode::FOUND, [("location", "/large")]) }),
            )
            .route(
                "/limited",
                get(|| async { (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1200")]) }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = http_client().unwrap();
        let response = client
            .get(format!("http://{address}/large"))
            .send()
            .await
            .unwrap();
        assert!(limited_body(response, 3).await.is_err());
        let response = client
            .get(format!("http://{address}/redirect"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert!(check_status(&response).is_err());
        let limiter = RequestLimiter::new();
        let requested_at = Instant::now();
        let error = limiter
            .send(client.get(format!("http://{address}/limited")))
            .await
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<RateLimited>()
                .unwrap()
                .retry_after_seconds,
            1200
        );
        server.abort();
        tokio::time::pause();
        limiter.wait().await;
        assert!(Instant::now() - requested_at >= Duration::from_secs(1200));
        let resumed_at = Instant::now();
        limiter.wait().await;
        assert!(Instant::now() - resumed_at > REQUEST_INTERVAL);
    }
}
