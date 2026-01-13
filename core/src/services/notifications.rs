use crate::services::shared::env::get_env_variable;
use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use log::error;
use log::info;
use serde::Serialize;

#[derive(Serialize)]
struct SendTelegramMessageRequest {
    chat_id: String,
    text: String,
    parse_mode: String,
}

async fn send_telegram_message(text: &str) -> Result<(), Error> {
    let token = get_env_variable("TG_TOKEN");
    let chat_id = get_env_variable("TG_CHAT_ID");

    match token {
        Some(token) => match chat_id {
            Some(chat_id) => {
                let client = reqwest::Client::new();

                let mut message_text = text.to_string();

                if cfg!(debug_assertions) {
                    message_text = format!("[🧪 DEV]\n {}", text);
                }

                let message = SendTelegramMessageRequest {
                    chat_id,
                    text: message_text,
                    parse_mode: "HTML".to_string(),
                };
                let res = client
                    .post(format!("https://api.telegram.org/bot{}/sendMessage", token))
                    .json(&message)
                    .send()
                    .await
                    .with_context(|| "Couldn't send Telegram message")?;

                let status = res.status();
                let body = res.text().await.unwrap_or_default();

                if !status.is_success() {
                    error!(
                        "Couldn't send message, Telegram API returned: {} with message: {}",
                        status, body
                    )
                }
            }
            None => {
                info!("TG_CHAT_ID (Telegram chat id) not set, didn't send Telegram notification.");
            }
        },
        None => {
            info!("TG_TOKEN (Telegram bot token) not set, didn't send Telegram notification.");
        }
    }

    Ok(())
}

pub struct Notification {
    pub content: String,
}

impl Notification {
    pub async fn send(&self) -> Result<(), Error> {
        send_telegram_message(&self.content).await?;
        Ok(())
    }
}
