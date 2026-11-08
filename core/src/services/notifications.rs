use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, GenericClient};

use crate::{
    database::{db_client, models::auth::UserId},
    services::shared::env::get_env_variable,
};

#[derive(Serialize)]
struct SendTelegramMessageRequest<'a> {
    chat_id: &'a str,
    text: &'a str,
    parse_mode: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    disable_web_page_preview: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
}

pub fn token_configured() -> bool {
    get_env_variable("TG_TOKEN").is_some_and(|value| !value.trim().is_empty())
}

pub async fn chat_id(user_id: &UserId, client: &impl GenericClient) -> Result<Option<String>> {
    Ok(client.query_opt(
        "SELECT telegram_chat_id FROM user_notification_settings WHERE user_id=$1",
        &[&user_id.as_str()],
    ).await?.and_then(|row| row.get(0)))
}

pub async fn set_chat_id(user_id: &UserId, client: &impl GenericClient, chat_id: Option<&str>) -> Result<()> {
    client.execute(
        "INSERT INTO user_notification_settings(user_id,telegram_chat_id) VALUES($1,$2)
         ON CONFLICT(user_id) DO UPDATE SET telegram_chat_id=EXCLUDED.telegram_chat_id",
        &[&user_id.as_str(), &chat_id],
    ).await?;
    Ok(())
}

async fn delivery_destination(user_id: &UserId, client: &impl GenericClient) -> Result<Option<String>> {
    Ok(client.query_opt(
        "SELECT n.telegram_chat_id FROM user_notification_settings n
         JOIN auth_users u ON u.id=n.user_id
         WHERE n.user_id=$1 AND u.status='active' AND u.username IS NOT NULL
           AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL",
        &[&user_id.as_str()],
    ).await?.and_then(|row| row.get::<_, Option<String>>(0))
        .filter(|value| !value.trim().is_empty()))
}

pub async fn configured(user_id: &UserId) -> Result<bool> {
    if !token_configured() { return Ok(false); }
    Ok(delivery_destination(user_id, &db_client().await?).await?.is_some())
}

async fn migrate_legacy_telegram_in(client: &mut Client, legacy_chat_id: Option<&str>) -> Result<()> {
    let transaction = client.transaction().await?;
    let row = transaction.query_one(
        "SELECT legacy_owner_user_id,legacy_telegram_migrated FROM auth_instance WHERE singleton=TRUE FOR UPDATE",
        &[],
    ).await?;
    if row.get::<_, bool>("legacy_telegram_migrated") { return Ok(()); }
    transaction.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('doughbox:legacy-telegram',0))", &[],
    ).await?;
    let user_id = UserId::from_database(row.get("legacy_owner_user_id"));
    let legacy_chat_id = legacy_chat_id.map(str::trim).filter(|value| !value.is_empty());
    transaction.execute(
        "INSERT INTO user_notification_settings(user_id,telegram_chat_id) VALUES($1,$2)
         ON CONFLICT(user_id) DO UPDATE SET telegram_chat_id=
             COALESCE(user_notification_settings.telegram_chat_id,EXCLUDED.telegram_chat_id)",
        &[&user_id.as_str(), &legacy_chat_id],
    ).await?;
    transaction.execute("UPDATE auth_instance SET legacy_telegram_migrated=TRUE WHERE singleton=TRUE", &[]).await?;
    transaction.commit().await?;
    Ok(())
}

/// Import TG_CHAT_ID once, before workers start. It is never a runtime fallback.
pub async fn migrate_legacy_telegram() -> Result<()> {
    migrate_legacy_telegram_in(&mut db_client().await?, get_env_variable("TG_CHAT_ID").as_deref()).await
}

async fn send_telegram_message_with(user_id: &UserId, text: &str, disable_preview: bool) -> Result<()> {
    let token = get_env_variable("TG_TOKEN")
        .filter(|value| !value.trim().is_empty())
        .context("Telegram bot token is not configured")?;
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
    // Recheck the original owner immediately before delivery. A disabled account
    // cannot send a report that was already being valued when it was disabled.
    let chat_id = delivery_destination(user_id, &db_client().await?).await?
        .context("Telegram destination is not configured or the account is inactive")?;
    send_to_endpoint(&endpoint, &chat_id, text, disable_preview).await
}

async fn send_to_endpoint(endpoint: &str, chat_id: &str, text: &str, disable_preview: bool) -> Result<()> {
    anyhow::ensure!(!chat_id.trim().is_empty(), "Telegram chat ID is not configured");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow::anyhow!("Could not initialize Telegram transport"))?;
    let response = client.post(endpoint)
        .json(&SendTelegramMessageRequest {
            chat_id,
            text,
            parse_mode: "HTML",
            disable_web_page_preview: disable_preview,
        })
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
    pub async fn send(&self, user_id: &UserId) -> Result<()> {
        send_telegram_message_with(user_id, &self.content, false).await
    }

    pub async fn send_without_preview(&self, user_id: &UserId) -> Result<()> {
        send_telegram_message_with(user_id, &self.content, true).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
    use crate::services::auth::tests::{cleanup, second_connection, test_database_at};

    async fn capture_delivery() -> (String, tokio::task::JoinHandle<serde_json::Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/botSECRET_TOKEN/sendMessage", listener.local_addr().unwrap());
        let request = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            let mut length = None;
            loop {
                use tokio::io::AsyncBufReadExt;
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" { break; }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
            }
            let mut body = vec![0; length.unwrap()];
            stream.read_exact(&mut body).await.unwrap();
            stream.get_mut().write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            ).await.unwrap();
            serde_json::from_slice(&body).unwrap()
        });
        (endpoint, request)
    }

    #[tokio::test]
    #[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
    async fn tenant_isolation_database_destinations_are_owned_and_legacy_import_is_atomic_once() {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let alice = crate::database::queries::auth::legacy_owner(&db).await.unwrap();
        db.execute(
            "UPDATE auth_users SET username='alice',password_hash='test-phc',totp_secret_encrypted='test-factor',status='active' WHERE id=$1",
            &[&alice.as_str()],
        ).await.unwrap();
        let bob = UserId::new();
        db.execute(
            "INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'bob','test-phc','test-factor','member','active')",
            &[&bob.as_str()],
        ).await.unwrap();
        let tx = db.transaction().await.unwrap();
        crate::database::queries::auth::initialize_private_defaults(&bob, &tx).await.unwrap();
        tx.commit().await.unwrap();
        db.batch_execute(
            "CREATE FUNCTION reject_legacy_telegram() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'injected persistence failure'; END $$;
             CREATE TRIGGER reject_legacy_telegram BEFORE UPDATE ON auth_instance
             FOR EACH ROW EXECUTE FUNCTION reject_legacy_telegram()",
        ).await.unwrap();
        assert!(migrate_legacy_telegram_in(&mut db, Some(" -100 ")).await.is_err());
        assert!(chat_id(&alice, &db).await.unwrap().is_none());
        assert!(!db.query_one("SELECT legacy_telegram_migrated FROM auth_instance WHERE singleton=TRUE", &[]).await.unwrap().get::<_, bool>(0));
        db.batch_execute("DROP TRIGGER reject_legacy_telegram ON auth_instance; DROP FUNCTION reject_legacy_telegram()").await.unwrap();
        let mut replica = second_connection(&schema).await;
        let (first, second) = tokio::join!(
            migrate_legacy_telegram_in(&mut db, Some(" -100 ")),
            migrate_legacy_telegram_in(&mut replica, Some(" -100 ")),
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(chat_id(&alice, &db).await.unwrap().as_deref(), Some("-100"));
        assert!(chat_id(&bob, &db).await.unwrap().is_none());
        assert!(delivery_destination(&bob, &db).await.unwrap().is_none());
        set_chat_id(&bob, &db, Some("-200")).await.unwrap();
        for (user, expected, preview) in [(&alice, "-100", false), (&bob, "-200", true)] {
            let destination = delivery_destination(user, &db).await.unwrap().unwrap();
            let (endpoint, received) = capture_delivery().await;
            send_to_endpoint(&endpoint, &destination, "own report", preview).await.unwrap();
            let request = received.await.unwrap();
            assert_eq!(request["chat_id"], expected);
            assert_eq!(request["text"], "own report");
            assert_eq!(request["disable_web_page_preview"].as_bool().unwrap_or(false), preview);
        }
        db.execute("UPDATE auth_users SET status='disabled' WHERE id=$1", &[&bob.as_str()]).await.unwrap();
        assert!(delivery_destination(&bob, &db).await.unwrap().is_none());
        assert_eq!(delivery_destination(&alice, &db).await.unwrap().as_deref(), Some("-100"));
        db.execute("UPDATE auth_users SET status='pending' WHERE id=$1", &[&bob.as_str()]).await.unwrap();
        assert!(delivery_destination(&bob, &db).await.unwrap().is_none());
        set_chat_id(&alice, &db, None).await.unwrap();
        migrate_legacy_telegram_in(&mut db, Some("-forbidden-fallback")).await.unwrap();
        assert!(chat_id(&alice, &db).await.unwrap().is_none());
        assert_eq!(chat_id(&bob, &db).await.unwrap().as_deref(), Some("-200"));
        assert!(db.query_one("SELECT legacy_telegram_migrated FROM auth_instance WHERE singleton=TRUE", &[]).await.unwrap().get::<_, bool>(0));
        drop(replica);
        cleanup(db, &schema).await;
    }

    #[tokio::test]
    #[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
    async fn tenant_isolation_database_legacy_telegram_preserves_existing_destination() {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let owner = crate::database::queries::auth::legacy_owner(&db).await.unwrap();
        set_chat_id(&owner, &db, Some("-existing")).await.unwrap();
        migrate_legacy_telegram_in(&mut db, Some("-legacy")).await.unwrap();
        assert_eq!(chat_id(&owner, &db).await.unwrap().as_deref(), Some("-existing"));
        cleanup(db, &schema).await;
    }

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
        send_to_endpoint(&endpoint, "private_chat", "Report", false).await.unwrap();
        for (status, body) in [
            ("200 OK", r#"{"ok":false,"description":"SECRET_TOKEN private_chat"}"#),
            ("200 OK", "SECRET_TOKEN malformed"),
            ("200 OK", r#"{"result":{}}"#),
            ("401 Unauthorized", "SECRET_TOKEN private_chat"),
            ("500 Internal Server Error", "SECRET_TOKEN private_chat"),
        ] {
            let endpoint = serve_response(status, body).await;
            let error = send_to_endpoint(&endpoint, "private_chat", "Report", false).await.unwrap_err();
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
        let error = send_to_endpoint(&endpoint, "private_chat", "Report", true).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(!message.contains("SECRET_TOKEN"));
        assert!(!message.contains("http://"));
        assert!(send_to_endpoint(&endpoint, " \t", "Report", false).await.is_err());
    }
}
