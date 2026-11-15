use super::*;
use crate::services::trading::rate_limits::*;
use axum::{routing::post,Router,Json,extract::State,http::StatusCode};
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};

struct Env(Option<std::ffi::OsString>);
impl Drop for Env {fn drop(&mut self){provider::set_test_broker_url(None).unwrap();match self.0.take(){Some(old)=>std::env::set_var("POSTGRES_URL",old),None=>std::env::remove_var("POSTGRES_URL")}}}
fn credentials(key:&str)->provider::Credentials {provider::Credentials{api_key:key.into(),api_secret:"fixture-secret".into()}}
fn context(user:&UserId,scope:BrokerRateScope)->BrokerRequestContext {BrokerRequestContext{scope,mode:AdmissionMode::Try,access:BrokerAccess::Owner{user_id:user.clone(),generation:"generation".into()}}}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_shared_budget_and_dispatch_safety(){
    let url=std::env::var("TEST_POSTGRES_URL").expect("isolated TEST_POSTGRES_URL required");
    let (mut db,connection)=tokio_postgres::connect(&url,NoTls).await.unwrap();tokio::spawn(connection);
    let schema=format!("t212_budget_{}",uuid::Uuid::new_v4().simple());
    db.batch_execute(&format!("CREATE SCHEMA {schema};SET search_path TO {schema}")).await.unwrap();
    refinery::embed_migrations!("migrations");migrations::runner().run_async(&mut db).await.unwrap();
    let mut scoped=reqwest::Url::parse(&url).unwrap();scoped.query_pairs_mut().append_pair("options",&format!("-csearch_path={schema}"));
    let _env=Env(std::env::var_os("POSTGRES_URL"));std::env::set_var("POSTGRES_URL",scoped.to_string().replace('+',"%20"));
    let alice=UserId::new();let bob=UserId::new();
    for (owner,name) in [(&alice,"alice"),(&bob,"bob")] {
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,$2,'fixture','fixture','member','active')",&[&owner.as_str(),&name]).await.unwrap();
        db.execute("INSERT INTO trading_connection(user_id,generation,environment,credentials,account) VALUES($1,'generation','live','fixture',$2)",&[&owner.as_str(),&json!({"id":"shared-account","currency":"EUR"})]).await.unwrap();
    }
    let account=json!({"id":"shared-account","currency":"EUR"});
    let scope=connection_scope("live",&account,&credentials("alice-key")).await.unwrap();
    let other_scope=connection_scope("live",&account,&credentials("bob-key")).await.unwrap();
    let a=BrokerBudget::new(context(&alice,scope.clone()));let b=BrokerBudget::new(context(&bob,other_scope));
    let (first,second)=tokio::join!(a.admit("export_create",true),b.admit("export_create",true));
    assert!(first.is_ok()^second.is_ok());
    let error=first.err().or_else(||second.err()).unwrap();assert!(error.downcast_ref::<BrokerRateLimited>().unwrap().retry_after_seconds>=30);
    assert!(b.admit("positions",true).await.is_ok());
    let independent=BrokerBudget::new(context(&alice,BrokerRateScope::account("live","different-account")));
    assert!(independent.admit("export_create",true).await.is_ok());
    let rotated=connection_scope("live",&account,&credentials("rotated-key")).await.unwrap();
    assert!(BrokerBudget::new(context(&alice,rotated)).admit("export_create",true).await.is_err());
    let provisional=BrokerRateScope::key("live",&key_hash(&credentials("unknown-key")));
    let provisional_budget=BrokerBudget::new(context(&alice,provisional.clone()));provisional_budget.admit("account_summary",true).await.unwrap();
    let promoted=promote_scope(&provisional,"shared-account",&key_hash(&credentials("unknown-key"))).await.unwrap();
    assert!(BrokerBudget::new(context(&alice,promoted)).admit("account_summary",true).await.is_err());
    let mut headers=reqwest::header::HeaderMap::new();headers.insert("retry-after","120".parse().unwrap());
    assert!(a.observe("positions",reqwest::StatusCode::TOO_MANY_REQUESTS,&headers).await.unwrap()>=119);
    headers.insert("retry-after","1".parse().unwrap());
    assert!(a.observe("positions",reqwest::StatusCode::OK,&headers).await.unwrap()>=119);
    // Two admitted real POSTs only after the broker definitely refuses the first.
    let posts=Arc::new(AtomicUsize::new(0));
    async fn order(State(count):State<Arc<AtomicUsize>>)->(StatusCode,Json<Value>){
        if count.fetch_add(1,Ordering::SeqCst)==0 {(StatusCode::TOO_MANY_REQUESTS,Json(json!({"message":"wait"})))}
        else {(StatusCode::OK,Json(json!({"id":9007199254740993_i64})))}
    }
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let app=Router::new().route("/api/v0/equity/orders/limit",post(order)).with_state(posts.clone());
    let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
    provider::set_test_broker_url(Some(format!("http://{address}/api/v0"))).unwrap();
    db.execute("INSERT INTO trading_proposals(id,user_id,generation,client_request_id,request,title,note,environment,status,trades) VALUES('execution',$1,'generation','request','{}','Fixture','','live','executing','[]')",&[&alice.as_str()]).await.unwrap();
    let broker=provider::BrokerClient::new("live",credentials("alice-key"),BrokerRequestContext{scope:scope.clone(),mode:AdmissionMode::Wait,access:BrokerAccess::Approved{user_id:alice.clone(),generation:"generation".into()}}).unwrap();
    let id=broker.submit_limit("FIXTURE_EQ","buy",rust_decimal::Decimal::ONE,rust_decimal::Decimal::ONE,"DAY").await.unwrap();
    assert_eq!(id,"9007199254740993");assert_eq!(posts.load(Ordering::SeqCst),2);
    // A background export admission never takes the order execution lock.
    assert!(db.query_one("SELECT pg_try_advisory_lock(hashtextextended('doughbox:trading:'||$1::text,0))",&[&alice.as_str()]).await.unwrap().get::<_,bool>(0));
    server.abort();let _=server.await;
    db.batch_execute(&format!("SET search_path TO public;DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
