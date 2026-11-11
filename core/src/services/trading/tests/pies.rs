//! Isolated PostgreSQL scenarios. Run this module in its own process with
//! TEST_POSTGRES_URL and --ignored --test-threads=1; never a portfolio database.
use super::*;
use crate::services::trading::pies::{
    self as service, PieAccess, PieApprovalInput, PieOperationInput, PieResult, PieSettingsChanges,
    PieStatus,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tokio_postgres::{Client, NoTls};

struct Env {
    previous: Vec<(&'static str, Option<OsString>)>,
}
impl Env {
    fn set(values: &[(&'static str, String)]) -> Self {
        let previous = values
            .iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                std::env::set_var(key, value);
                (*key, old)
            })
            .collect();
        Self { previous }
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        provider::set_test_broker_url(None).unwrap();
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
#[derive(Default)]
struct BrokerState {
    pies: BTreeMap<String, Value>,
    writes: Vec<Value>,
    reads: usize,
    block_write: bool,
}
#[derive(Clone, Default)]
struct Stub {
    state: Arc<Mutex<BrokerState>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
fn detail(id: &str, a: &str, b: &str, quantity: Option<&str>) -> Value {
    json!({"settings":{"id":id.parse::<i64>().unwrap(),"name":"Original Pie","icon":"original","dividendCashAction":"REINVEST","goal":1000,"endDate":"legacy-date","instrumentShares":{"AAPL_US_EQ":serde_json::from_str::<Value>(a).unwrap(),"MSFT_US_EQ":serde_json::from_str::<Value>(b).unwrap()}},"instruments":[{"ticker":"AAPL_US_EQ","ownedQuantity":quantity.map(|q|serde_json::from_str::<Value>(q).unwrap()),"currentShare":0.55,"expectedShare":0.6},{"ticker":"MSFT_US_EQ","ownedQuantity":0,"currentShare":0.45,"expectedShare":0.4}]})
}
async fn list(State(stub): State<Stub>) -> Json<Value> {
    Json(Value::Array(
        stub.state
            .lock()
            .unwrap()
            .pies
            .keys()
            .map(|id| json!({"id":id.parse::<i64>().unwrap()}))
            .collect(),
    ))
}
async fn read(State(stub): State<Stub>, Path(id): Path<String>) -> Response {
    let mut state = stub.state.lock().unwrap();
    state.reads += 1;
    state
        .pies
        .get(&id)
        .cloned()
        .map(|v| Json(v).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
async fn create(State(stub): State<Stub>, Json(body): Json<Value>) -> Response {
    let (value, blocked) = {
        let mut state = stub.state.lock().unwrap();
        state
            .writes
            .push(json!({"method":"POST","path":"/equity/pies","body":body}));
        let id = (100 + state.writes.len()).to_string();
        let mut settings = body;
        settings["id"] = json!(id.parse::<i64>().unwrap());
        let value = json!({"settings":settings,"instruments":[]});
        state.pies.insert(id, value.clone());
        (value, state.block_write)
    };
    if blocked {
        stub.entered.notify_one();
        stub.release.notified().await;
    }
    Json(value).into_response()
}
async fn update(
    State(stub): State<Stub>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut state = stub.state.lock().unwrap();
    state
        .writes
        .push(json!({"method":"POST","path":format!("/equity/pies/{id}"),"body":body}));
    let Some(value) = state.pies.get_mut(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut settings = body;
    settings["id"] = json!(id.parse::<i64>().unwrap());
    value["settings"] = settings;
    Json(value.clone()).into_response()
}
async fn delete(State(stub): State<Stub>, Path(id): Path<String>) -> Response {
    let mut state = stub.state.lock().unwrap();
    state
        .writes
        .push(json!({"method":"DELETE","path":format!("/equity/pies/{id}")}));
    state.pies.remove(&id);
    (StatusCode::OK, "deleted").into_response()
}
struct Fixture {
    db: Client,
    schema: String,
    user: UserId,
    stub: Stub,
    server: tokio::task::JoinHandle<()>,
    _env: Env,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_POSTGRES_URL").expect("isolated TEST_POSTGRES_URL required");
        let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let schema = format!("pie_test_{}", uuid::Uuid::new_v4().simple());
        db.batch_execute(&format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        ))
        .await
        .unwrap();
        refinery::embed_migrations!("migrations");
        migrations::runner().run_async(&mut db).await.unwrap();
        let mut scoped = reqwest::Url::parse(&url).unwrap();
        scoped
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let env = Env::set(&[
            ("POSTGRES_URL", scoped.to_string().replace('+', "%20")),
            ("TRADING_MASTER_KEY", "19".repeat(32)),
        ]);
        let user = UserId::new();
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'pie-owner','fixture-password','fixture-totp','member','active')",&[&user.as_str()]).await.unwrap();
        let encrypted = provider::encrypt_credentials(&provider::Credentials {
            api_key: "dummy-key".into(),
            api_secret: "dummy-secret".into(),
        })
        .unwrap();
        let catalog = json!([{"ticker":"AAPL_US_EQ","name":"Apple","currency":"USD","isin":"US0378331005"},{"ticker":"MSFT_US_EQ","name":"Microsoft","currency":"USD","isin":"US5949181045"}]);
        db.execute("INSERT INTO trading_connection(user_id,generation,environment,credentials,instruments,agent_token_hash,agent_auth_version) VALUES($1,'test','demo',$2,$3,'agent',1)",&[&user.as_str(),&encrypted,&catalog]).await.unwrap();
        let stub = Stub::default();
        stub.state
            .lock()
            .unwrap()
            .pies
            .insert("10".into(), detail("10", "0.6", "0.4", Some("0")));
        let routes = Router::new()
            .route("/equity/pies", get(list).post(create))
            .route("/equity/pies/{id}", get(read).post(update).delete(delete));
        let router = Router::new()
            .nest("/api/v0", routes)
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        provider::set_test_broker_url(Some(format!("http://{address}/api/v0"))).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            db,
            schema,
            user,
            stub,
            server,
            _env: env,
        }
    }
    async fn fresh_gate(&self) {
        self.db.execute("UPDATE trading_connection SET pie_request_next_at='epoch',pie_list_next_at='epoch' WHERE user_id=$1",&[&self.user.as_str()]).await.unwrap();
    }
    async fn load(&self, id: &str) -> Proposal {
        Proposal::from_row(
            &self
                .db
                .query_one(
                    "SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$2",
                    &[&self.user.as_str(), &id],
                )
                .await
                .unwrap(),
        )
        .unwrap()
    }
    async fn close(self) {
        self.server.abort();
        self.db
            .batch_execute(&format!(
                "SET search_path TO public; DROP SCHEMA {} CASCADE",
                self.schema
            ))
            .await
            .unwrap();
    }
}
fn input(key: &str, operation: PieOperationInput) -> service::SubmitPieInput {
    service::SubmitPieInput {
        client_request_id: key.into(),
        title: "Owner review".into(),
        note: String::new(),
        operation,
    }
}
fn create_input(key: &str) -> service::SubmitPieInput {
    serde_json::from_value(json!({"client_request_id":key,"title":"Create Pie","operation":{"action":"create","settings":{"name":"New Pie","dividend_cash_action":"REINVEST","instrument_shares":{"AAPL_US_EQ":"0.7","MSFT_US_EQ":"0.3"}}}})).unwrap()
}
fn update_input(key: &str) -> service::SubmitPieInput {
    input(
        key,
        PieOperationInput::Update {
            pie_id: "10".into(),
            changes: PieSettingsChanges {
                name: Some("Renamed Pie".into()),
                instrument_shares: Some(BTreeMap::from([
                    ("AAPL_US_EQ".into(), "0.7".into()),
                    ("MSFT_US_EQ".into(), "0.3".into()),
                ])),
                ..Default::default()
            },
        },
    )
}
fn pie(proposal: &Proposal) -> &PieResult {
    let ProposalContent::Pie { pie } = &proposal.content else {
        panic!("expected Pie")
    };
    pie
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn concurrent_keys_approvals_and_cross_kind_collisions_never_duplicate_dispatch() {
    let fixture = Fixture::new().await;
    let (a, b) = tokio::join!(
        service::submit(
            &fixture.user,
            PieAccess::Agent("agent"),
            create_input("same")
        ),
        service::submit(&fixture.user, PieAccess::Browser, create_input("same"))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.proposal.id, b.proposal.id);
    assert_ne!(a.created, b.created);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 0);
    let mut changed = create_input("same");
    changed.title = "different".into();
    assert!(service::submit(&fixture.user, PieAccess::Browser, changed)
        .await
        .is_err());
    let mut changed_action = create_input("same");
    changed_action.operation = PieOperationInput::Delete {
        pie_id: "10".into(),
    };
    assert!(
        service::submit(&fixture.user, PieAccess::Browser, changed_action)
            .await
            .is_err()
    );
    assert_eq!(fixture.stub.state.lock().unwrap().reads, 0);
    let trade_request = SubmitInput {
        client_request_id: "same".into(),
        title: "Create Pie".into(),
        note: String::new(),
        trades: vec![TradeInput {
            ticker: "AAPL_US_EQ".into(),
            side: "buy".into(),
            quantity: "1".into(),
            limit_price: "1".into(),
            time_validity: "DAY".into(),
        }],
    };
    assert!(submit(&fixture.user, "agent", trade_request).await.is_err());
    let (a, b) = tokio::join!(
        approve(
            &fixture.user,
            a.proposal.id.clone(),
            PieApprovalInput::default()
        ),
        approve(
            &fixture.user,
            a.proposal.id.clone(),
            PieApprovalInput::default()
        )
    );
    assert!(a.is_ok() ^ b.is_ok());
    let result = fixture.load(&a.as_ref().or(b.as_ref()).unwrap().id).await;
    assert_eq!(pie(&result).status, PieStatus::Confirmed);
    assert!(pie(&result).broker_pie_id.is_some());
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 1);
    let replay = service::submit(&fixture.user, PieAccess::Browser, create_input("same"))
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.proposal.id, result.id);
    assert!(pie(&result).error.is_none());
    fixture.fresh_gate().await;
    let checked = service::reconcile(&fixture.user, &result.id).await.unwrap();
    assert_eq!(pie(&checked).status, PieStatus::Confirmed);
    assert!(pie(&checked).error.is_none());
    assert!(pie(&fixture.load(&result.id).await).error.is_none());
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 1);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn source_drift_delete_acknowledgement_and_missing_holdings_block_writes() {
    let fixture = Fixture::new().await;
    let update = service::submit(&fixture.user, PieAccess::Browser, update_input("drift"))
        .await
        .unwrap();
    fixture
        .stub
        .state
        .lock()
        .unwrap()
        .pies
        .insert("10".into(), detail("10", "0.8", "0.2", Some("0")));
    fixture.fresh_gate().await;
    let result = approve(
        &fixture.user,
        update.proposal.id,
        PieApprovalInput::default(),
    )
    .await
    .unwrap();
    assert_eq!(pie(&result).status, PieStatus::Conflict);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 0);
    fixture.fresh_gate().await;
    let deletion = service::submit(
        &fixture.user,
        PieAccess::Browser,
        input(
            "delete",
            PieOperationInput::Delete {
                pie_id: "10".into(),
            },
        ),
    )
    .await
    .unwrap();
    assert!(approve(
        &fixture.user,
        deletion.proposal.id.clone(),
        PieApprovalInput::default()
    )
    .await
    .is_err());
    assert_eq!(fixture.load(&deletion.proposal.id).await.status, "pending");
    let result = approve(
        &fixture.user,
        deletion.proposal.id,
        PieApprovalInput {
            confirm_empty_pie: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(pie(&result).status, PieStatus::Confirmed);
    assert!(pie(&result).empty_confirmed_by_owner);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 1);
    for quantity in [None, Some("0.01")] {
        fixture
            .stub
            .state
            .lock()
            .unwrap()
            .pies
            .insert("10".into(), detail("10", "0.6", "0.4", quantity));
        fixture.fresh_gate().await;
        assert!(service::submit(
            &fixture.user,
            PieAccess::Browser,
            input(
                &format!("blocked-{quantity:?}"),
                PieOperationInput::Delete {
                    pie_id: "10".into()
                }
            )
        )
        .await
        .is_err());
    }
    fixture
        .stub
        .state
        .lock()
        .unwrap()
        .pies
        .insert("10".into(), detail("10", "0.6", "0.4", Some("0")));
    fixture.fresh_gate().await;
    let deletion = service::submit(
        &fixture.user,
        PieAccess::Browser,
        input(
            "holdings-drift",
            PieOperationInput::Delete {
                pie_id: "10".into(),
            },
        ),
    )
    .await
    .unwrap();
    fixture
        .stub
        .state
        .lock()
        .unwrap()
        .pies
        .insert("10".into(), detail("10", "0.6", "0.4", Some("1")));
    fixture.fresh_gate().await;
    let result = approve(
        &fixture.user,
        deletion.proposal.id,
        PieApprovalInput {
            confirm_empty_pie: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(pie(&result).status, PieStatus::Conflict);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 1);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn recovery_reconnect_and_order_reconciliation_preserve_selected_payload() {
    let mut fixture = Fixture::new().await;
    fixture.db.execute(
        "UPDATE trading_connection SET account=$2 WHERE user_id=$1",
        &[&fixture.user.as_str(), &json!({"id":"eur-account","currency":"EUR"})],
    ).await.unwrap();
    let proposal = service::submit(&fixture.user, PieAccess::Browser, create_input("recovery"))
        .await
        .unwrap()
        .proposal;
    assert_eq!(pie(&proposal).account_currency.as_deref(), Some("EUR"));
    fixture.db.execute("UPDATE trading_proposals SET status='executing',pie=jsonb_set(pie,'{status}','\"submitting\"') WHERE id=$1",&[&proposal.id]).await.unwrap();
    assert!(store::execution_lock(&fixture.user, &fixture.db)
        .await
        .unwrap());
    store::recover_on(&fixture.user, &fixture.db).await.unwrap();
    let recovered = fixture.load(&proposal.id).await;
    assert_eq!(recovered.status, "completed");
    assert_eq!(pie(&recovered).status, PieStatus::Unknown);
    let before = serde_json::to_value(pie(&recovered)).unwrap();
    let reconciled = store::reconcile_order_statuses(
        &fixture.user,
        &fixture.db,
        &proposal.id,
        &std::collections::HashMap::from([("101".into(), "filled".into())]),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(pie(&reconciled)).unwrap(), before);
    fixture.db.execute("UPDATE trading_proposals SET status='executing',pie=jsonb_set(pie,'{status}','\"submitted\"') WHERE id=$1",&[&proposal.id]).await.unwrap();
    store::recover_on(&fixture.user, &fixture.db).await.unwrap();
    assert_eq!(
        pie(&fixture.load(&proposal.id).await).status,
        PieStatus::Submitted
    );
    let pending = service::submit(&fixture.user, PieAccess::Browser, create_input("pending"))
        .await
        .unwrap()
        .proposal;
    let connection = fixture.db.query_one(
        "SELECT credentials,instruments FROM trading_connection WHERE user_id=$1",
        &[&fixture.user.as_str()],
    ).await.unwrap();
    let credentials: String = connection.get("credentials");
    let instruments: Value = connection.get("instruments");
    let tx = fixture.db.transaction().await.unwrap();
    store::replace_connection_on(&fixture.user, &tx, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let discarded = fixture.load(&pending.id).await;
    assert_eq!(discarded.status, "discarded");
    assert_eq!(pie(&discarded).status, PieStatus::Discarded);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 0);
    let tx = fixture.db.transaction().await.unwrap();
    store::replace_connection_on(
        &fixture.user,
        &tx,
        Some(("demo", &credentials, &json!({"id":"usd-account","currency":"USD"}), &instruments)),
    ).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(pie(&fixture.load(&proposal.id).await).account_currency.as_deref(), Some("EUR"));
    assert_eq!(pie(&fixture.load(&pending.id).await).account_currency.as_deref(), Some("EUR"));
    let next = service::submit(&fixture.user, PieAccess::Browser, create_input("new-currency"))
        .await.unwrap().proposal;
    assert_eq!(pie(&next).account_currency.as_deref(), Some("USD"));
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn terminated_execution_session_cancels_readback_and_recovers_unknown_without_replay() {
    let fixture = Fixture::new().await;
    let proposal = service::submit(
        &fixture.user,
        PieAccess::Browser,
        create_input("session-loss"),
    )
    .await
    .unwrap()
    .proposal;
    fixture.stub.state.lock().unwrap().block_write = true;
    let user = fixture.user.clone();
    let id = proposal.id.clone();
    let task = tokio::spawn(async move { approve(&user, id, PieApprovalInput::default()).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.stub.entered.notified(),
    )
    .await
    .unwrap();
    let row=fixture.db.query_one("SELECT pg_terminate_backend(pid) FROM pg_locks WHERE locktype='advisory' AND granted AND classid::bigint=((hashtextextended('doughbox:trading:' || $1::text,0)>>32)&4294967295) AND objid::bigint=(hashtextextended('doughbox:trading:' || $1::text,0)&4294967295)",&[&fixture.user.as_str()]).await.unwrap();
    assert!(row.get::<_, bool>(0));
    assert!(task.await.unwrap().is_err());
    fixture.stub.release.notify_one();
    store::recover_interrupted(&fixture.user).await.unwrap();
    let recovered = fixture.load(&proposal.id).await;
    assert_eq!(pie(&recovered).status, PieStatus::Unknown);
    let replay = service::submit(
        &fixture.user,
        PieAccess::Browser,
        create_input("session-loss"),
    )
    .await
    .unwrap();
    assert!(!replay.created);
    assert!(
        approve(&fixture.user, proposal.id, PieApprovalInput::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 1);
    assert_eq!(fixture.stub.state.lock().unwrap().reads, 0);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn shared_pacing_claim_is_atomic_owner_and_generation_scoped() {
    let fixture = Fixture::new().await;
    let other = reconciliation::connect_schema(&fixture.schema).await;
    let (a, b) = tokio::join!(
        store::claim_pie_request(
            &fixture.user,
            &fixture.db,
            "test",
            store::PieBucket::Request
        ),
        store::claim_pie_request(&fixture.user, &other, "test", store::PieBucket::Request)
    );
    assert!(a.is_ok() ^ b.is_ok());
    let error = a.err().or_else(|| b.err()).unwrap();
    assert!((1..=6).contains(
        &error
            .downcast_ref::<service::PieRateLimited>()
            .unwrap()
            .retry_after_seconds
    ));
    assert!(
        store::claim_pie_request(&fixture.user, &fixture.db, "old", store::PieBucket::List)
            .await
            .is_err()
    );
    store::extend_pie_rate_limit(
        &fixture.user,
        &fixture.db,
        "test",
        store::PieBucket::Request,
        100,
    )
    .await
    .unwrap();
    let error = store::claim_pie_request(&fixture.user, &other, "test", store::PieBucket::Request)
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<service::PieRateLimited>()
            .unwrap()
            .retry_after_seconds
            >= 99
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn matching_known_target_observation_never_attributes_unknown_dispatch() {
    let fixture = Fixture::new().await;
    let proposal = service::submit(
        &fixture.user,
        PieAccess::Browser,
        update_input("unknown-target"),
    )
    .await
    .unwrap()
    .proposal;
    fixture.db.execute("UPDATE trading_proposals SET status='completed',pie=jsonb_set(pie,'{status}','\"unknown\"') WHERE id=$1",&[&proposal.id]).await.unwrap();
    fixture
        .stub
        .state
        .lock()
        .unwrap()
        .pies
        .insert("10".into(), detail("10", "0.7", "0.3", Some("0")));
    fixture.fresh_gate().await;
    let observed = service::reconcile(&fixture.user, &proposal.id)
        .await
        .unwrap();
    assert_eq!(pie(&observed).status, PieStatus::Unknown);
    let evidence = pie(&observed).observation.as_ref().unwrap();
    assert_eq!(evidence.state, service::PieObservationState::Present);
    assert_eq!(
        evidence
            .pie
            .as_ref()
            .unwrap()
            .settings
            .as_ref()
            .unwrap()
            .instrument_shares
            .as_ref()
            .unwrap()["AAPL_US_EQ"]
            .as_deref(),
        Some("0.7")
    );
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 0);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "isolated TEST_POSTGRES_URL; dedicated process --test-threads=1"]
async fn malformed_source_id_conflicts_without_alternate_lookup_or_dispatch() {
    let fixture = Fixture::new().await;
    let proposal = service::submit(
        &fixture.user,
        PieAccess::Browser,
        update_input("malformed-source"),
    )
    .await
    .unwrap()
    .proposal;
    fixture
        .stub
        .state
        .lock()
        .unwrap()
        .pies
        .insert("10".into(), detail("99", "0.6", "0.4", Some("0")));
    fixture.fresh_gate().await;
    let result = approve(&fixture.user, proposal.id, PieApprovalInput::default())
        .await
        .unwrap();
    assert_eq!(pie(&result).status, PieStatus::Conflict);
    assert_eq!(fixture.stub.state.lock().unwrap().writes.len(), 0);
    assert_eq!(fixture.stub.state.lock().unwrap().reads, 2);
    fixture.close().await;
}
