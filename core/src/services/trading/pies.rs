use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

// Missing means preserve/unspecified. Explicit null never means clear a broker setting.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub enum DividendCashAction {
    REINVEST,
    #[serde(rename = "TO_ACCOUNT_CASH")]
    ToAccountCash,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PieCreateSettings {
    pub name: String,
    pub dividend_cash_action: DividendCashAction,
    pub instrument_shares: BTreeMap<String, String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub icon: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub goal: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub end_date: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PieSettingsChanges {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DividendCashAction")]
    #[schema(value_type = DividendCashAction, nullable = false)]
    pub dividend_cash_action: Option<DividendCashAction>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, String>")]
    #[schema(value_type = BTreeMap<String, String>, nullable = false)]
    pub instrument_shares: Option<BTreeMap<String, String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub icon: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub goal: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    #[schema(value_type = String, nullable = false)]
    pub end_date: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PieOperationInput {
    Create {
        settings: PieCreateSettings,
    },
    Update {
        pie_id: String,
        changes: PieSettingsChanges,
    },
    Duplicate {
        pie_id: String,
        name: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "String")]
        #[schema(value_type = String, nullable = false)]
        icon: Option<String>,
    },
    Delete {
        pie_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitPieInput {
    pub client_request_id: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    pub operation: PieOperationInput,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparedPieMutation {
    Create {
        settings: PieCreateSettings,
    },
    Update {
        pie_id: String,
        settings: PieCreateSettings,
    },
    Duplicate {
        pie_id: String,
        name: String,
        icon: Option<String>,
    },
    Delete {
        pie_id: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieSettings {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub dividend_cash_action: Option<String>,
    pub goal: Option<String>,
    pub end_date: Option<String>,
    pub instrument_shares: Option<BTreeMap<String, Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_shares_source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieValueResult {
    pub price_avg_invested_value: Option<String>,
    pub price_avg_result: Option<String>,
    pub price_avg_result_coef: Option<String>,
    pub price_avg_value: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieDividendDetails {
    pub gained: Option<String>,
    pub in_cash: Option<String>,
    pub reinvested: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieSummary {
    pub id: String,
    pub cash: Option<String>,
    pub dividend_details: Option<PieDividendDetails>,
    pub progress: Option<String>,
    pub result: Option<PieValueResult>,
    pub status: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieList {
    pub fetched_at: DateTime<Utc>,
    pub items: Vec<PieSummary>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieIssue {
    pub name: Option<String>,
    pub severity: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieInstrument {
    pub ticker: Option<String>,
    pub owned_quantity: Option<String>,
    pub current_share: Option<String>,
    pub expected_share: Option<String>,
    pub issues: Option<Vec<PieIssue>>,
    pub result: Option<PieValueResult>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieDetail {
    pub fetched_at: DateTime<Utc>,
    pub id: String,
    pub settings: Option<PieSettings>,
    pub instruments: Option<Vec<PieInstrument>>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PieStatus {
    Pending,
    Submitting,
    Submitted,
    Confirmed,
    Conflict,
    Failed,
    Unknown,
    Discarded,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PieObservationState {
    Present,
    Absent,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieObservation {
    pub observed_at: DateTime<Utc>,
    pub state: PieObservationState,
    pub pie: Option<PieDetail>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct PieResult {
    pub mutation: PreparedPieMutation,
    /// Account currency at preparation time; unknown for older saved proposals.
    #[serde(default)]
    pub account_currency: Option<String>,
    pub before: Option<PieDetail>,
    pub expected_settings: Option<PieSettings>,
    pub status: PieStatus,
    pub broker_pie_id: Option<String>,
    pub error: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub observation: Option<PieObservation>,
    pub empty_confirmed_by_owner: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PieApprovalInput {
    #[serde(default)]
    pub confirm_empty_pie: bool,
}

#[derive(Debug)]
pub struct PieRateLimited {
    pub retry_after_seconds: u64,
}
impl std::fmt::Display for PieRateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Trading Pie request rate limited; retry after {} seconds.",
            self.retry_after_seconds
        )
    }
}
impl std::error::Error for PieRateLimited {}

#[derive(Clone, Copy)]
pub enum PieAccess<'a> {
    Browser,
    Agent(&'a str),
}

pub(super) mod domain;

use super::{
    provider::{self, pies::PieBrokerError, BrokerClient, Instrument},
    store::{self, PieBucket},
    Proposal, ProposalContent, SubmitOutcome,
};
use crate::{
    database::{db_client, models::auth::UserId},
    services::auth::AuthError,
};
use anyhow::{ensure, Context};
use tokio_postgres::{Client, Row, Transaction};

struct Connection {
    generation: String,
    environment: String,
    encrypted: String,
}

async fn connection_on(
    user_id: &UserId,
    tx: &Transaction<'_>,
    access: PieAccess<'_>,
) -> anyhow::Result<Row> {
    match access {
        PieAccess::Browser => store::browser_connection(user_id, tx).await,
        PieAccess::Agent(hash) => store::agent_connection(user_id, tx, hash).await,
    }
}

fn snapshot(row: &Row) -> Connection {
    Connection {
        generation: row.get("generation"),
        environment: row.get("environment"),
        encrypted: row.get("credentials"),
    }
}

async fn authorize(
    user_id: &UserId,
    client: &mut Client,
    access: PieAccess<'_>,
    generation: &str,
) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    let row = connection_on(user_id, &tx, access).await?;
    ensure!(
        row.get::<_, &str>("generation") == generation,
        "Trading connection changed. Refresh before continuing."
    );
    tx.commit().await?;
    Ok(())
}

async fn connection(
    user_id: &UserId,
    client: &mut Client,
    access: PieAccess<'_>,
    generation: Option<&str>,
) -> anyhow::Result<Connection> {
    let tx = client.transaction().await?;
    let row = connection_on(user_id, &tx, access).await?;
    if let Some(generation) = generation {
        ensure!(
            row.get::<_, &str>("generation") == generation,
            "Trading connection changed. Refresh before continuing."
        );
    }
    let current = snapshot(&row);
    tx.commit().await?;
    Ok(current)
}

async fn gate(
    user_id: &UserId,
    client: &mut Client,
    access: PieAccess<'_>,
    generation: &str,
    bucket: PieBucket,
    wait: bool,
) -> anyhow::Result<()> {
    loop {
        authorize(user_id, client, access, generation).await?;
        match store::claim_pie_request(user_id, client, generation, bucket).await {
            Ok(()) => {
                authorize(user_id, client, access, generation).await?;
                return Ok(());
            }
            Err(error) => {
                if wait {
                    if let Some(rate) = error.downcast_ref::<PieRateLimited>() {
                        tokio::time::sleep(std::time::Duration::from_secs(
                            rate.retry_after_seconds,
                        ))
                        .await;
                        continue;
                    }
                }
                return Err(error);
            }
        }
    }
}

async fn read_error(
    user_id: &UserId,
    client: &Client,
    generation: &str,
    bucket: PieBucket,
    error: PieBrokerError,
) -> anyhow::Error {
    if let PieBrokerError::RateLimited {
        retry_after_seconds,
        ..
    } = error
    {
        if let Err(error) =
            store::extend_pie_rate_limit(user_id, client, generation, bucket, retry_after_seconds)
                .await
        {
            return error;
        }
        return PieRateLimited {
            retry_after_seconds,
        }
        .into();
    }
    error.into()
}

async fn read_detail(
    user_id: &UserId,
    client: &mut Client,
    access: PieAccess<'_>,
    generation: &str,
    broker: &BrokerClient,
    id: &str,
    wait: bool,
) -> anyhow::Result<Option<PieDetail>> {
    gate(
        user_id,
        client,
        access,
        generation,
        PieBucket::Request,
        wait,
    )
    .await?;
    let outcome = broker.pie(id).await;
    authorize(user_id, client, access, generation).await?;
    match outcome {
        Ok(Some(mut detail)) => {
            let _ = domain::fill_missing_targets(&mut detail);
            Ok(Some(detail))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(read_error(user_id, client, generation, PieBucket::Request, error).await),
    }
}

pub async fn list(user_id: &UserId, access: PieAccess<'_>) -> anyhow::Result<PieList> {
    super::check_setup()?;
    let mut client = db_client().await?;
    let current = connection(user_id, &mut client, access, None).await?;
    let broker = BrokerClient::new(
        &current.environment,
        provider::decrypt_credentials(&current.encrypted)?,
    )?;
    gate(
        user_id,
        &mut client,
        access,
        &current.generation,
        PieBucket::List,
        false,
    )
    .await?;
    let outcome = broker.pies().await;
    connection(user_id, &mut client, access, Some(&current.generation)).await?;
    match outcome {
        Ok(list) => Ok(list),
        Err(error) => Err(read_error(
            user_id,
            &client,
            &current.generation,
            PieBucket::List,
            error,
        )
        .await),
    }
}

pub async fn detail(
    user_id: &UserId,
    access: PieAccess<'_>,
    pie_id: &str,
) -> anyhow::Result<PieDetail> {
    super::check_setup()?;
    domain::validate_id(pie_id)?;
    let mut client = db_client().await?;
    let current = connection(user_id, &mut client, access, None).await?;
    let broker = BrokerClient::new(
        &current.environment,
        provider::decrypt_credentials(&current.encrypted)?,
    )?;
    read_detail(
        user_id,
        &mut client,
        access,
        &current.generation,
        &broker,
        pie_id,
        false,
    )
    .await?
    .ok_or_else(|| AuthError::NotFound.into())
}

async fn replay(
    user_id: &UserId,
    tx: &Transaction<'_>,
    generation: &str,
    input: &SubmitPieInput,
) -> anyhow::Result<Option<SubmitOutcome>> {
    let row = tx.query_opt("SELECT * FROM trading_proposals WHERE user_id=$1 AND generation=$2 AND client_request_id=$3", &[&user_id.as_str(), &generation, &input.client_request_id]).await?;
    row.map(|row| {
        ensure!(row.get::<_, String>("kind") == "pie" && row.get::<_, serde_json::Value>("request") == serde_json::to_value(input)?, "Idempotency conflict: client_request_id already has different kind, operation or text.");
        Ok(SubmitOutcome { proposal: Proposal::from_row(&row)?, created: false })
    }).transpose()
}

pub async fn submit(
    user_id: &UserId,
    access: PieAccess<'_>,
    input: SubmitPieInput,
) -> anyhow::Result<SubmitOutcome> {
    super::check_setup()?;
    super::validate_envelope(&input.client_request_id, &input.title, &input.note)?;
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    let row = connection_on(user_id, &tx, access).await?;
    let current = snapshot(&row);
    if let Some(outcome) = replay(user_id, &tx, &current.generation, &input).await? {
        return Ok(outcome);
    }
    let instruments: Vec<Instrument> = serde_json::from_value(row.get("instruments"))
        .context("Saved instrument catalog is incomplete; reconnect Trading 212.")?;
    tx.commit().await?;
    domain::validate_operation(&input.operation, &instruments)?;
    let broker = BrokerClient::new(
        &current.environment,
        provider::decrypt_credentials(&current.encrypted)?,
    )?;
    let source = match &input.operation {
        PieOperationInput::Create { .. } => None,
        PieOperationInput::Update { pie_id, .. }
        | PieOperationInput::Duplicate { pie_id, .. }
        | PieOperationInput::Delete { pie_id } => {
            let mut source = read_detail(user_id, &mut client, access, &current.generation, &broker, pie_id, false).await?.context("Source Pie no longer exists.")?;
            domain::fill_missing_targets(&mut source)?;
            Some(source)
        }
    };
    let mut pie = domain::prepare(input.operation.clone(), source)?;
    let tx = client.transaction().await?;
    let checked = connection_on(user_id, &tx, access).await?;
    ensure!(
        checked.get::<_, &str>("generation") == current.generation,
        "Trading connection changed. Submit a new proposal."
    );
    if let Some(outcome) = replay(user_id, &tx, &current.generation, &input).await? {
        return Ok(outcome);
    }
    pie.account_currency = checked
        .get::<_, Option<serde_json::Value>>("account")
        .and_then(|account| account.get("currency")?.as_str().map(str::to_owned));
    let id = uuid::Uuid::new_v4().to_string();
    let row = tx.query_one(
        "INSERT INTO trading_proposals(user_id,id,generation,client_request_id,request,title,note,environment,kind,trades,pie) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pie','[]'::jsonb,$9) RETURNING *",
        &[&user_id.as_str(), &id, &current.generation, &input.client_request_id, &serde_json::to_value(&input)?, &input.title, &input.note, &current.environment, &serde_json::to_value(pie)?],
    ).await?;
    let proposal = Proposal::from_row(&row)?;
    tx.commit().await?;
    Ok(SubmitOutcome {
        proposal,
        created: true,
    })
}

fn source_id(mutation: &PreparedPieMutation) -> Option<&str> {
    match mutation {
        PreparedPieMutation::Create { .. } => None,
        PreparedPieMutation::Update { pie_id, .. }
        | PreparedPieMutation::Duplicate { pie_id, .. }
        | PreparedPieMutation::Delete { pie_id } => Some(pie_id),
    }
}

fn known_target(pie: &PieResult) -> Option<&str> {
    pie.broker_pie_id
        .as_deref()
        .or_else(|| match &pie.mutation {
            PreparedPieMutation::Update { pie_id, .. } | PreparedPieMutation::Delete { pie_id } => {
                Some(pie_id.as_str())
            }
            _ => None,
        })
}

async fn observe(
    user_id: &UserId,
    client: &mut Client,
    broker: &BrokerClient,
    generation: &str,
    pie: &mut PieResult,
) -> anyhow::Result<()> {
    let Some(id) = known_target(pie).map(str::to_owned) else {
        pie.error = Some("No broker Pie ID was returned. Check Trading 212 manually; this request will not be resent and copies will not be identified by name.".into());
        return Ok(());
    };
    match read_detail(
        user_id,
        client,
        PieAccess::Browser,
        generation,
        broker,
        &id,
        true,
    )
    .await
    {
        Ok(actual) => {
            let matches = match &actual {
                Some(actual) => {
                    !matches!(pie.mutation, PreparedPieMutation::Delete { .. })
                        && domain::result_matches(pie, actual)
                }
                None => matches!(pie.mutation, PreparedPieMutation::Delete { .. }),
            };
            pie.observation = Some(PieObservation {
                observed_at: Utc::now(),
                state: if actual.is_some() {
                    PieObservationState::Present
                } else {
                    PieObservationState::Absent
                },
                pie: actual,
            });
            if matches!(pie.status, PieStatus::Submitted | PieStatus::Confirmed) && matches {
                pie.status = PieStatus::Confirmed;
                pie.error = None;
            } else if pie.status == PieStatus::Unknown {
                pie.error = Some("Broker state observed, but dispatch attribution remains unknown. Matching settings or absence do not prove this request executed. Check Trading 212 before proposing another mutation.".into());
            } else {
                pie.error = Some("The acknowledged request is not yet verified: observed broker configuration does not match the approved result. No request was resent.".into());
            }
        }
        Err(error) => {
            pie.error = Some(format!(
                "Broker verification unavailable: {error}. No mutation was retried."
            ));
        }
    }
    Ok(())
}

pub(super) async fn execute(
    user_id: &UserId,
    client: &mut Client,
    broker: &BrokerClient,
    generation: &str,
    id: &str,
    pie: &mut PieResult,
) -> anyhow::Result<()> {
    if let Some(source_id) = source_id(&pie.mutation).map(str::to_owned) {
        match read_detail(
            user_id,
            client,
            PieAccess::Browser,
            generation,
            broker,
            &source_id,
            true,
        )
        .await
        {
            Ok(Some(mut actual)) => {
                let _ = domain::fill_missing_targets(&mut actual);
                let configuration_ok = pie.before.as_ref().is_some_and(|before| domain::configuration_matches(before, &actual));
                let holdings_ok = !matches!(pie.mutation, PreparedPieMutation::Delete { .. })
                    || domain::ensure_empty(&actual).is_ok();
                if !configuration_ok || !holdings_ok {
                    pie.status = PieStatus::Conflict;
                    pie.error = Some("Source settings changed, are incomplete, or deletion holdings are no longer known empty. Nothing was sent. Prepare a fresh proposal.".into());
                } else if let PreparedPieMutation::Update { pie_id, settings } = &pie.mutation {
                    if domain::keeps_current_name(
                        actual.settings.as_ref().and_then(|settings| settings.name.as_deref()),
                        &settings.name,
                    ) {
                        pie.status = PieStatus::Failed;
                        pie.error = Some(format!(
                            "{} Nothing was sent. Prepare a fresh proposal.",
                            domain::unchanged_name_update(pie_id)
                        ));
                    }
                }
            }
            Ok(None) => {
                pie.status = PieStatus::Conflict;
                pie.error = Some("Source Pie no longer exists. Nothing was sent.".into());
            }
            Err(error) => {
                pie.status = if matches!(
                    error.downcast_ref::<PieBrokerError>(),
                    Some(PieBrokerError::MalformedResponse(_))
                ) {
                    PieStatus::Conflict
                } else {
                    PieStatus::Failed
                };
                pie.error = Some(format!(
                    "Preflight could not verify the source Pie: {error}. Nothing was sent."
                ));
            }
        }
        if pie.status != PieStatus::Pending {
            store::save_pie(user_id, client, id, pie).await?;
            return Ok(());
        }
    }
    gate(
        user_id,
        client,
        PieAccess::Browser,
        generation,
        PieBucket::Request,
        true,
    )
    .await?;
    pie.status = PieStatus::Submitting;
    store::save_pie(user_id, client, id, pie).await?;
    // One dispatch only. A remote application can still race after preflight.
    let outcome = match &pie.mutation {
        PreparedPieMutation::Create { settings } => {
            broker.create_pie(settings).await.map(|detail| detail.id)
        }
        PreparedPieMutation::Update { pie_id, settings } => broker
            .update_pie(pie_id, settings)
            .await
            .map(|()| pie_id.clone()),
        PreparedPieMutation::Duplicate { pie_id, name, icon } => broker
            .duplicate_pie(pie_id, name, icon.as_deref())
            .await
            .map(|detail| detail.id),
        PreparedPieMutation::Delete { pie_id } => {
            broker.delete_pie(pie_id).await.map(|()| pie_id.clone())
        }
    };
    match outcome {
        Ok(id) => {
            pie.status = PieStatus::Submitted;
            pie.broker_pie_id = Some(id);
            pie.submitted_at = Some(Utc::now());
        }
        Err(error) => {
            pie.status = match &error {
                PieBrokerError::Unknown(_) | PieBrokerError::MalformedResponse(_) => {
                    PieStatus::Unknown
                }
                _ => PieStatus::Failed,
            };
            pie.error = Some(error.to_string());
            if let PieBrokerError::RateLimited {
                retry_after_seconds,
                ..
            } = error
            {
                // Never resend a mutation, including a definite HTTP 429 rejection.
                store::extend_pie_rate_limit(
                    user_id,
                    client,
                    generation,
                    PieBucket::Request,
                    retry_after_seconds,
                )
                .await?;
            }
        }
    }
    store::save_pie(user_id, client, id, pie).await?;
    if pie.status == PieStatus::Submitted {
        observe(user_id, client, broker, generation, pie).await?;
        store::save_pie(user_id, client, id, pie).await?;
    } else if pie.status == PieStatus::Unknown && known_target(pie).is_none() {
        pie.error = Some(format!("{} Check Trading 212 manually; no broker ID was returned. Do not retry blindly. This request will not be resent.", pie.error.as_deref().unwrap_or("Outcome unknown.")));
        store::save_pie(user_id, client, id, pie).await?;
    }
    Ok(())
}

pub async fn reconcile(user_id: &UserId, proposal_id: &str) -> anyhow::Result<Proposal> {
    let (mut client, session) = crate::database::db_connection().await?;
    tokio::pin!(session);
    let work = async {
        ensure!(
            store::execution_lock(user_id, &client).await?,
            "Another proposal is executing. Wait for it to finish."
        );
        store::recover_on(user_id, &client).await?;
        let current = connection(user_id, &mut client, PieAccess::Browser, None).await?;
        let row = client
            .query_opt(
                "SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$2 AND generation=$3",
                &[&user_id.as_str(), &proposal_id, &current.generation],
            )
            .await?
            .ok_or(AuthError::NotFound)?;
        let mut proposal = Proposal::from_row(&row)?;
        let ProposalContent::Pie { pie } = &mut proposal.content else {
            anyhow::bail!("Only Pie proposals support broker-state checks.");
        };
        ensure!(
            proposal.status == "completed"
                && matches!(
                    pie.status,
                    PieStatus::Submitted | PieStatus::Unknown | PieStatus::Confirmed
                ),
            "This Pie request has no dispatched outcome to check."
        );
        let broker = BrokerClient::new(
            &current.environment,
            provider::decrypt_credentials(&current.encrypted)?,
        )?;
        observe(user_id, &mut client, &broker, &current.generation, pie).await?;
        store::save_pie(user_id, &client, proposal_id, pie).await?;
        Ok(proposal)
    };
    tokio::pin!(work);
    tokio::select! {
        biased;
        _ = &mut session => anyhow::bail!("Trading execution database session was lost; no further work was performed."),
        result = &mut work => result,
    }
}
