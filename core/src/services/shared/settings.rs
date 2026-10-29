use chrono::{DateTime, Utc};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tracing_subscriber::filter::LevelFilter;
use typeshare::typeshare;
use utoipa::ToSchema;

use super::env::get_env_variable;

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TelegramReportScheduleSettings {
    pub enabled: bool,
    /// Strict 24-hour HH:MM, always UTC.
    pub send_time_utc: String,
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct TelegramReportScheduleStatus {
    pub enabled: bool,
    pub send_time_utc: String,
    pub next_report_at: Option<DateTime<Utc>>,
    pub last_sent_at: Option<DateTime<Utc>>,
    /// not_run, running, succeeded, or failed.
    pub last_status: String,
    pub last_error: Option<String>,
}

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ApplicationSettings {
    pub log_level: String,
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct ApplicationSettingsStatus {
    pub log_level: String,
    /// Whether TG_TOKEN is non-empty; never includes the credential itself.
    pub telegram_token_set: bool,
    /// Whether TG_CHAT_ID is non-empty; does not validate the chat with Telegram.
    pub telegram_chat_id_set: bool,
    pub telegram_schedule: TelegramReportScheduleStatus,
}

impl ApplicationSettingsStatus {
    pub async fn load(settings: ApplicationSettings) -> anyhow::Result<Self> {
        let db = crate::database::db_client().await?;
        crate::database::queries::telegram_schedule::recover_interrupted(&db).await?;
        let telegram_schedule = crate::database::queries::telegram_schedule::read(&db).await?;
        Ok(Self {
            log_level: settings.log_level,
            telegram_token_set: get_env_variable("TG_TOKEN")
                .is_some_and(|value| !value.trim().is_empty()),
            telegram_chat_id_set: get_env_variable("TG_CHAT_ID")
                .is_some_and(|value| !value.trim().is_empty()),
            telegram_schedule,
        })
    }
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
