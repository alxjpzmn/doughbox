use anyhow::ensure;
use tokio_postgres::Client;

use crate::services::shared::settings::{parse_log_level, ApplicationSettings};

pub async fn read(client: &Client) -> anyhow::Result<ApplicationSettings> {
    let row = client
        .query_one(
            "SELECT log_level FROM application_settings WHERE singleton",
            &[],
        )
        .await?;
    Ok(ApplicationSettings {
        log_level: row.get(0),
    })
}

pub async fn update(client: &Client, settings: &ApplicationSettings) -> anyhow::Result<()> {
    parse_log_level(&settings.log_level)?;
    let level = settings.log_level.to_ascii_uppercase();
    let changed = client
        .execute(
            "UPDATE application_settings SET log_level = $1 WHERE singleton",
            &[&level],
        )
        .await?;
    ensure!(
        changed == 1,
        "Application settings have not been initialized"
    );
    Ok(())
}
