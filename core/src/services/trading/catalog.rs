use anyhow::{anyhow, ensure};

use super::{
    provider::{self, BrokerClient, Instrument},
    store,
};
use crate::database::db_client;

pub(super) struct Catalog {
    pub generation: String,
    pub instruments: Vec<Instrument>,
}

pub(super) async fn load(hash: &str) -> anyhow::Result<Catalog> {
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    let connection = store::agent_connection(&tx, hash).await?;
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
    // Claim before HTTP, including failed attempts, without keeping the row locked over HTTP.
    // Sixty seconds leaves margin over Trading 212's documented 1 request / 50s limit.
    let claimed = tx.execute(
        "UPDATE trading_connection SET last_instruments_request_at=clock_timestamp() WHERE singleton AND generation=$1 AND agent_token_hash=$2 AND credentials IS NOT NULL AND last_instruments_request_at <= clock_timestamp() - interval '60 seconds'",
        &[&generation, &hash],
    ).await?;
    ensure!(
        claimed == 1,
        "Trading instrument catalog ISIN backfill is already in progress or rate limited. Retry after at least 60 seconds."
    );
    tx.commit().await?;

    let broker = BrokerClient::new(&environment, provider::decrypt_credentials(&encrypted)?)?;
    let instruments = broker.instruments().await.map_err(|error| {
        anyhow!(
        "Trading instrument catalog ISIN backfill failed: {error}. Retry after at least 60 seconds."
    )
    })?;
    ensure!(
        !instruments.is_empty(),
        "Trading 212 returned no supported instruments during catalog ISIN backfill. Retry after at least 60 seconds."
    );
    // Never replace the connection: its generation, agent token and proposals stay intact.
    // A reconnect or token rotation/revocation during HTTP invalidates this response.
    let saved = client.execute(
        "UPDATE trading_connection SET instruments=$3 WHERE singleton AND generation=$1 AND agent_token_hash=$2 AND credentials IS NOT NULL",
        &[&generation, &hash, &serde_json::to_value(&instruments)?],
    ).await?;
    ensure!(
        saved == 1,
        "Trading connection changed or token was revoked during instrument catalog backfill. Retry with the current trading token."
    );
    Ok(Catalog {
        generation,
        instruments,
    })
}
