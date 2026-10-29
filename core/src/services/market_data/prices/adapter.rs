use std::collections::HashSet;

use chrono::{DateTime, Utc};
use reqwest::Client;

use super::{
    cboe,
    config::Feed,
    deutsche_boerse, euronext,
    types::{Download, PriceReport, SourceFile},
};

pub(super) async fn discover(client: &Client, feed: Feed) -> anyhow::Result<Vec<SourceFile>> {
    match feed {
        Feed::Xetra | Feed::Tradegate | Feed::Frankfurt => {
            deutsche_boerse::discover(client, feed).await
        }
        Feed::Cboe => cboe::discover(client).await,
        Feed::Euronext => euronext::discover(client).await,
    }
}

pub(super) async fn download(
    client: &Client,
    feed: Feed,
    file: &SourceFile,
    etag: Option<&str>,
) -> anyhow::Result<Option<Download>> {
    match feed {
        Feed::Xetra | Feed::Tradegate | Feed::Frankfurt => {
            deutsche_boerse::download(client, file, etag).await
        }
        Feed::Cboe => cboe::download(client, file, etag).await,
        Feed::Euronext => euronext::download(client, file, etag).await,
    }
}

pub(super) fn parse(
    feed: Feed,
    file: &SourceFile,
    bytes: &[u8],
    tracked: &HashSet<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<PriceReport>> {
    match feed {
        Feed::Xetra | Feed::Tradegate | Feed::Frankfurt => {
            deutsche_boerse::parse_gzip(feed, bytes, tracked, now)
        }
        Feed::Cboe => cboe::parse(file, bytes, tracked, now),
        Feed::Euronext => euronext::parse(file, bytes, tracked, now),
    }
}
