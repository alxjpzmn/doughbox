use chrono::{DateTime, Utc};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tracing_subscriber::filter::LevelFilter;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::{
    database::{db_client, models::auth::UserId, queries::telegram_schedule},
    services::notifications,
};

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ApplicationSettings {
    pub log_level: String,
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct ApplicationSettingsStatus {
    /// Whether the instance-operated TG_TOKEN is non-empty; never its value.
    pub telegram_token_set: bool,
    /// This user's destination only. None means notifications are unconfigured.
    pub telegram_chat_id: Option<String>,
    pub telegram_schedule: TelegramReportScheduleStatus,
}

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TelegramNotificationSettings {
    pub chat_id: Option<String>,
}

impl TelegramNotificationSettings {
    pub fn normalized_chat_id(&self) -> anyhow::Result<Option<String>> {
        self.chat_id.as_deref().map(|value| {
            let value = value.trim();
            anyhow::ensure!(
                !value.is_empty() && value.chars().count() <= 64,
                "Telegram chat ID must contain 1 to 64 characters, or be null to disconnect"
            );
            Ok(value.to_owned())
        }).transpose()
    }
}

impl ApplicationSettingsStatus {
    pub async fn load(user_id: &UserId) -> anyhow::Result<Self> {
        let db = db_client().await?;
        telegram_schedule::recover_interrupted(user_id, &db).await?;
        Ok(Self {
            telegram_token_set: notifications::token_configured(),
            telegram_chat_id: notifications::chat_id(user_id, &db).await?,
            telegram_schedule: telegram_schedule::read(user_id, &db).await?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_destination_is_trimmed_bounded_and_nullable() {
        let disconnected = TelegramNotificationSettings { chat_id: None };
        assert_eq!(disconnected.normalized_chat_id().unwrap(), None);
        let settings = TelegramNotificationSettings { chat_id: Some("  -12345 \n".into()) };
        assert_eq!(settings.normalized_chat_id().unwrap().as_deref(), Some("-12345"));
        for invalid in ["".to_owned(), " \t\n".to_owned(), "a".repeat(65)] {
            assert!(TelegramNotificationSettings { chat_id: Some(invalid) }.normalized_chat_id().is_err());
        }
        assert!(TelegramNotificationSettings { chat_id: Some("a".repeat(64)) }.normalized_chat_id().is_ok());
        assert!(serde_json::from_str::<TelegramNotificationSettings>(r#"{"chat_id":"123","user_id":"other"}"#).is_err());
        assert!(serde_json::from_str::<ApplicationSettings>(r#"{"log_level":"INFO","telegram_chat_id":"123"}"#).is_err());
        assert!(serde_json::from_str::<TelegramReportScheduleSettings>(r#"{"enabled":true,"send_time_utc":"18:00","user_id":"other"}"#).is_err());
    }

    #[test]
    fn member_settings_payload_has_only_own_notification_contract() {
        let status = ApplicationSettingsStatus {
            telegram_token_set: false,
            telegram_chat_id: Some("-own-chat".into()),
            telegram_schedule: TelegramReportScheduleStatus {
                enabled: false,
                send_time_utc: "18:00".into(),
                next_report_at: None,
                last_sent_at: None,
                last_status: "not_run".into(),
                last_error: None,
            },
        };
        let value = serde_json::to_value(status).unwrap();
        let fields = value.as_object().unwrap();
        assert_eq!(fields.len(), 3);
        assert!(fields.contains_key("telegram_token_set"));
        assert_eq!(fields["telegram_chat_id"], "-own-chat");
        assert!(fields.contains_key("telegram_schedule"));
        assert!(!fields.contains_key("log_level"));
    }
}
