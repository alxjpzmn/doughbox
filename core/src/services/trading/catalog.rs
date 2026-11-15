use anyhow::{ensure, Context};

use super::{
    provider::{self, Instrument},
    rate_limits::{AdmissionMode, BrokerAccess}, store,
};
use crate::database::{db_client, models::auth::UserId};

pub(super) struct Catalog {
    pub generation: String,
    pub instruments: Vec<Instrument>,
}

pub(super) async fn load(user_id: &UserId, hash: &str) -> anyhow::Result<Catalog> {
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    let connection = store::agent_connection(user_id, &tx, hash).await?;
    let generation: String = connection.get("generation");
    if let Ok(instruments) =
        serde_json::from_value::<Vec<Instrument>>(connection.get("instruments"))
    {
        if !instruments.is_empty()
            && instruments
                .iter()
                .all(|instrument| !instrument.isin.trim().is_empty())
        {
            tx.commit().await?;
            return Ok(Catalog {
                generation,
                instruments,
            });
        }
    }

    let environment: String = connection.get("environment");
    let encrypted: String = connection.get("credentials");
    let account = connection.get::<_, Option<serde_json::Value>>("account").context("Saved broker account is missing; reconnect Trading 212.")?;
    tx.commit().await?;

    let broker = super::broker_client(&environment, provider::decrypt_credentials(&encrypted)?, &account, AdmissionMode::Try, BrokerAccess::Agent { user_id: user_id.clone(), generation: generation.clone(), token_hash: hash.to_owned() }).await?;
    let instruments = broker.instruments().await.context("Trading instrument catalog ISIN backfill failed.")?;
    ensure!(
        !instruments.is_empty(),
        "Trading 212 returned no supported instruments during catalog ISIN backfill. Retry after at least 60 seconds."
    );
    // Never replace the connection: its generation, agent token and proposals stay intact.
    // A reconnect or token rotation/revocation during HTTP invalidates this response.
    let tx = client.transaction().await?;
    let connection = store::agent_connection(user_id, &tx, hash).await?;
    ensure!(
        connection.get::<_, String>("generation") == generation,
        "Trading connection changed or token was revoked during instrument catalog backfill. Retry with the current trading token."
    );
    tx.execute(
        "UPDATE trading_connection SET instruments=$3 WHERE user_id=$1 AND generation=$2",
        &[&user_id.as_str(), &generation, &serde_json::to_value(&instruments)?],
    ).await?;
    tx.commit().await?;
    Ok(Catalog {
        generation,
        instruments,
    })
}
