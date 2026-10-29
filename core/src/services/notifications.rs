use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::services::shared::env::get_env_variable;

#[derive(Serialize)]
struct SendTelegramMessageRequest<'a> {
    chat_id: &'a str,
    text: &'a str,
    parse_mode: &'static str,
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
}

pub fn configured() -> bool {
    ["TG_TOKEN", "TG_CHAT_ID"].iter().all(|name| {
        get_env_variable(name).is_some_and(|value| !value.trim().is_empty())
    })
}

async fn send_telegram_message(text: &str) -> Result<()> {
    let token = get_env_variable("TG_TOKEN")
        .filter(|value| !value.trim().is_empty())
        .context("Telegram bot token is not configured")?;
    let chat_id = get_env_variable("TG_CHAT_ID")
        .filter(|value| !value.trim().is_empty())
        .context("Telegram chat ID is not configured")?;
    // The endpoint is never configurable in production. No request URL, response
    // body, or credential-bearing reqwest error leaves this transport.
    let endpoint = format!("https://api.telegram.org/bot{token}/sendMessage");
    let dev_text;
    let text = if cfg!(debug_assertions) {
        dev_text = format!("[🧪 DEV]\n {text}");
        dev_text.as_str()
    } else {
        text
    };
    send_to_endpoint(&endpoint, &chat_id, text).await
}

async fn send_to_endpoint(endpoint: &str, chat_id: &str, text: &str) -> Result<()> {
    anyhow::ensure!(!chat_id.trim().is_empty(), "Telegram chat ID is not configured");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow::anyhow!("Could not initialize Telegram transport"))?;
    let response = client.post(endpoint)
        .json(&SendTelegramMessageRequest { chat_id, text, parse_mode: "HTML" })
        .send().await
        .map_err(|_| anyhow::anyhow!("Telegram request failed or timed out"))?;
    if !response.status().is_success() {
        bail!("Telegram rejected the message (HTTP {})", response.status().as_u16());
    }
    let body = response.json::<TelegramResponse>().await
        .map_err(|_| anyhow::anyhow!("Telegram returned an invalid delivery confirmation"))?;
    anyhow::ensure!(body.ok, "Telegram did not confirm message delivery");
    Ok(())
}

pub struct Notification {
    pub content: String,
}

impl Notification {
    pub async fn send(&self) -> Result<()> {
        send_telegram_message(&self.content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    async fn serve_response(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{address}/botSECRET_TOKEN/sendMessage")
    }

    #[tokio::test]
    async fn delivery_requires_http_and_telegram_confirmation() {
        let endpoint = serve_response("200 OK", r#"{"ok":true,"result":{"message_id":1}}"#).await;
        send_to_endpoint(&endpoint, "private_chat", "Report").await.unwrap();
        for (status, body) in [
            ("200 OK", r#"{"ok":false,"description":"SECRET_TOKEN private_chat"}"#),
            ("200 OK", "SECRET_TOKEN malformed"),
            ("200 OK", r#"{"result":{}}"#),
            ("401 Unauthorized", "SECRET_TOKEN private_chat"),
            ("500 Internal Server Error", "SECRET_TOKEN private_chat"),
        ] {
            let endpoint = serve_response(status, body).await;
            let error = send_to_endpoint(&endpoint, "private_chat", "Report").await.unwrap_err();
            let message = format!("{error:#}");
            assert!(!message.contains("SECRET_TOKEN"));
            assert!(!message.contains("private_chat"));
            assert!(!message.contains("http://"));
        }
    }

    #[tokio::test]
    async fn network_errors_hide_urls_and_blank_chat_ids_are_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/botSECRET_TOKEN/sendMessage", listener.local_addr().unwrap());
        drop(listener);
        let error = send_to_endpoint(&endpoint, "private_chat", "Report").await.unwrap_err();
        let message = format!("{error:#}");
        assert!(!message.contains("SECRET_TOKEN"));
        assert!(!message.contains("http://"));
        assert!(send_to_endpoint(&endpoint, " \t", "Report").await.is_err());
    }
}
