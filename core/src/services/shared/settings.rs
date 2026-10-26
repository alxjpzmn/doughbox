use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tracing_subscriber::filter::LevelFilter;
use typeshare::typeshare;
use utoipa::ToSchema;

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ApplicationSettings {
    pub log_level: String,
}

pub fn parse_log_level(value: &str) -> anyhow::Result<LevelFilter> {
    let level = value
        .parse::<LevelFilter>()
        .context("Unsupported log level; use ERROR, WARN, INFO, DEBUG or TRACE")?;
    if level == LevelFilter::OFF {
        bail!("Unsupported log level; use ERROR, WARN, INFO, DEBUG or TRACE");
    }
    Ok(level)
}
