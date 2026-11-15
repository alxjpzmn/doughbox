use super::*;
use crate::services::trading::{sync as service, provider::history::download::set_test_download_url};
use axum::{extract::State,routing::get,Router,Json,http::StatusCode,response::{Response,IntoResponse}};
use chrono::TimeDelta;
use std::sync::{Arc,Mutex};

struct Env(Vec<(&'static str,Option<std::ffi::OsString>)>);
impl Drop for Env {fn drop(&mut self){provider::set_test_broker_url(None).unwrap();set_test_download_url(None).unwrap();for(k,v) in self.0.drain(..){match v{Some(v)=>std::env::set_var(k,v),None=>std::env::remove_var(k)}}}}
#[derive(Default)]
struct BrokerState {request:Option<Value>,posts:usize,reads:usize,download_origin:String,lost_ack:bool,missing:bool,reject_range:bool,rate_once:bool,forbidden:bool,forbidden_body:Option<String>,reports_forbidden:bool,history_fill:Option<String>,history_transaction:Option<Value>,report_status:Option<&'static str>,csv:Vec<u8>}
#[derive(Clone,Default)]
struct Stub(Arc<Mutex<BrokerState>>);
async fn create(State(stub):State<Stub>,Json(request):Json<Value>)->Response {
    let mut state=stub.0.lock().unwrap();state.posts+=1;state.request=Some(request);
    if state.forbidden {
        let body=state.forbidden_body.clone().unwrap_or_else(|| r#"{"detail":"Scope(history:orders) missing for API key"}"#.to_owned());
        return (StatusCode::FORBIDDEN,body).into_response();
    }
    if state.reject_range {return StatusCode::BAD_REQUEST.into_response();}
    if state.rate_once {state.rate_once=false;return (StatusCode::TOO_MANY_REQUESTS,[("retry-after","120")]).into_response();}
    if state.lost_ack {state.lost_ack=false;return StatusCode::SERVICE_UNAVAILABLE.into_response();}
    Json(json!({"reportId":9007199254740993_i64})).into_response()
}
async fn reports(State(stub):State<Stub>)->Response{
    let mut state=stub.0.lock().unwrap();state.reads+=1;
    if state.reports_forbidden {
        return (StatusCode::FORBIDDEN,state.forbidden_body.clone().unwrap_or_else(|| r#"{"detail":"Scope(history:orders) missing for API key"}"#.to_owned())).into_response();
    }
    if state.missing{return Json(json!([])).into_response();}
    let Some(request)=state.request.as_ref() else{return Json(json!([])).into_response();};
    let status=state.report_status.unwrap_or(match state.reads {1=>"Queued",2=>"Processing",_=>"Finished"});
    Json(json!([{"reportId":9007199254740993_i64,"timeFrom":request["timeFrom"],"timeTo":request["timeTo"],"dataIncluded":request["dataIncluded"],"status":status,"downloadLink":format!("{}/report.csv?signed=fixture",state.download_origin)}])).into_response()
}
async fn download(State(stub):State<Stub>,headers:axum::http::HeaderMap)->Response {
    assert!(!headers.contains_key("authorization"));assert!(!headers.contains_key("cookie"));
    ([("content-type","text/csv")],stub.0.lock().unwrap().csv.clone()).into_response()
}
async fn summary()->Json<Value>{Json(json!({"id":"history-account","currency":"EUR","cash":{"availableToTrade":1000,"inPies":0,"reservedForOrders":0},"investments":{"currentValue":0,"totalCost":0,"unrealizedProfitLoss":0},"totalValue":1000}))}
async fn positions()->Json<Value>{Json(json!([]))}
async fn catalog()->Json<Value>{Json(json!([{"ticker":"AAPL_US_EQ","name":"Apple fixture","currencyCode":"EUR","isin":"US0378331005","type":"STOCK"}]))}
async fn pending()->Json<Value>{Json(json!([]))}
async fn order_history(State(stub):State<Stub>)->Json<Value>{
    let state=stub.0.lock().unwrap();
    let Some(time)=state.history_fill.clone() else {return Json(json!({"items":[]}));};
    Json(json!({"items":[{"fill":{"filledAt":time,"id":42,"price":10,"quantity":2,"type":"TRADE","walletImpact":{"currency":"EUR","fxRate":1,"taxes":[]}},"order":{"id":7,"side":"BUY","ticker":"AAPL_US_EQ","instrument":{"isin":"US0378331005","currency":"EUR","ticker":"AAPL_US_EQ","name":"Apple"}}}],"nextPagePath":null}))
}
async fn empty_history()->Json<Value>{Json(json!({"items":[]}))}
async fn transaction_history(State(stub):State<Stub>)->Json<Value>{
    let state=stub.0.lock().unwrap();
    Json(json!({"items":state.history_transaction.iter().collect::<Vec<_>>()}))
}
struct Fixture {db:tokio_postgres::Client,schema:String,user:UserId,account:String,stub:Stub,server:tokio::task::JoinHandle<()>,_env:Env}
impl Fixture {
    async fn new()->Self {
        let url=std::env::var("TEST_POSTGRES_URL").expect("isolated PostgreSQL required");
        let (mut db,connection)=tokio_postgres::connect(&url,NoTls).await.unwrap();tokio::spawn(connection);
        let schema=format!("t212_runtime_{}",uuid::Uuid::new_v4().simple());db.batch_execute(&format!("CREATE SCHEMA {schema};SET search_path TO {schema}")).await.unwrap();
        refinery::embed_migrations!("migrations");migrations::runner().run_async(&mut db).await.unwrap();
        let mut scoped=reqwest::Url::parse(&url).unwrap();scoped.query_pairs_mut().append_pair("options",&format!("-csearch_path={schema}"));
        let values=[("POSTGRES_URL",scoped.to_string().replace('+',"%20")),("TRADING_MASTER_KEY","29".repeat(32))];
        let env=Env(values.into_iter().map(|(k,v)|{let old=std::env::var_os(k);std::env::set_var(k,v);(k,old)}).collect());
        let user=UserId::new();let account=uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'runtime-owner','fixture','fixture','member','active')",&[&user.as_str()]).await.unwrap();
        let tx=db.transaction().await.unwrap();auth::initialize_private_defaults(&user,&tx).await.unwrap();tx.commit().await.unwrap();
        let encrypted=provider::encrypt_credentials(&provider::Credentials{api_key:"history-fixture".into(),api_secret:"history-secret".into()}).unwrap();
        db.execute("UPDATE trading_connection SET generation='fixture-generation',environment='live',credentials=$2,account=$3 WHERE user_id=$1",&[&user.as_str(),&encrypted,&json!({"id":"history-account","currency":"EUR"})]).await.unwrap();
        let opening=Utc::now()-TimeDelta::days(14);
        db.execute("INSERT INTO t212_sync_account(user_id,id,broker_account_id,currency,backfill_cursor) VALUES($1,$2,'history-account','EUR',$3)",&[&user.as_str(),&account,&opening]).await.unwrap();
        let time=(opening+TimeDelta::seconds(1)).to_rfc3339();
        let csv=format!("Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate\nInterest on cash,{time},,,,,1.20,EUR,cash-1,0,1\nDeposit,{time},,,,,20,EUR,deposit,0,1\n").into_bytes();
        let stub=Stub::default();stub.0.lock().unwrap().csv=csv;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();stub.0.lock().unwrap().download_origin=format!("http://{addr}");
        let app=Router::new().route("/api/v0/equity/history/exports",get(reports).post(create)).route("/report.csv",get(download))
            .route("/api/v0/equity/account/summary",get(summary)).route("/api/v0/equity/positions",get(positions))
            .route("/api/v0/equity/metadata/instruments",get(catalog)).route("/api/v0/equity/orders",get(pending))
            .route("/api/v0/equity/history/orders",get(order_history)).route("/api/v0/equity/history/dividends",get(empty_history)).route("/api/v0/equity/history/transactions",get(transaction_history)).route("/api/v0/equity/pies",get(positions)).with_state(stub.clone());
        let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap()});
        provider::set_test_broker_url(Some(format!("http://{addr}/api/v0"))).unwrap();set_test_download_url(Some(format!("http://{addr}/"))).unwrap();
        Self{db,schema,user,account,stub,server,_env:env}
    }
    async fn ready(&self){self.db.batch_execute("UPDATE t212_sync_run SET next_attempt_at='epoch' WHERE status IN ('queued','running','waiting');UPDATE trading_rate_limit SET next_at='epoch',blocked_until='epoch'").await.unwrap();}
    async fn step(&self){let(_stop,rx)=tokio::sync::watch::channel(false);service::run_test_session(&self.user,&self.account,rx).await.unwrap();}
    async fn cleanup(self){self.server.abort();let _=self.server.await;self.db.batch_execute(&format!("SET search_path TO public;DROP SCHEMA {} CASCADE",self.schema)).await.unwrap();}
    async fn five_events(&mut self) {
        let time=(Utc::now()-TimeDelta::days(10)).format("%Y-%m-%d %H:%M:%S").to_string();
        let csv=format!("Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate,Withholding tax,Currency (Withholding tax),Notes\nMarket buy,{time},US0378331005,2,10,EUR,20,EUR,order-1,0,1,0,EUR,\nDividend (Ordinary),{time},US0378331005,2,3,EUR,5,EUR,dividend-1,0,1,1,EUR,\nInterest on cash,{time},,,,,1.20,EUR,cash-1,0,1,0,EUR,\nLending interest,{time},,,,,0.30,EUR,loan-1,0,1,0,EUR,\nCurrency conversion,{time},,,,,9,EUR,fx-1,0,1,0,EUR,10 USD -> 9 EUR\nDeposit,{time},,,,,100,EUR,deposit,0,1,0,EUR,\nWithdrawal,{time},,,,,20,EUR,withdrawal,0,1,0,EUR,\n").into_bytes();
        let prepared=crate::services::importers::trading212::prepare_trading212_csv(&self.user,&csv).await.unwrap();
        for row in &prepared.rows {
            if let crate::services::importers::trading212::T212Financial::Dividend(dividend)=&row.financial {
                crate::database::queries::dividend::insert_with_id(&self.user,&self.db,"pre-feature-dividend",dividend).await.unwrap();
            }
        }
        self.stub.0.lock().unwrap().csv=csv;
    }
    async fn browser_session(&mut self)->(Arc<crate::services::auth::AuthService>,crate::services::auth::session_store::IssuedSession) {
        use crate::services::auth::{AuthConfig,AuthService,session_store::{create_anonymous,issue_successor_locked,SessionGrant}};
        let service=AuthService::new(AuthConfig::parse("http://localhost:5173",&"17".repeat(32),true).unwrap());
        let secret=service.encrypt_totp(&self.user,false,b"12345678901234567890").unwrap();
        self.db.execute("UPDATE auth_users SET totp_secret_encrypted=$2 WHERE id=$1",&[&self.user.as_str(),&secret]).await.unwrap();
        let user=auth::by_id(&self.user,&self.db).await.unwrap().unwrap();let now=Utc::now();
        let tx=self.db.transaction().await.unwrap();let anonymous=create_anonymous(&tx,now).await.unwrap();
        let session=issue_successor_locked(&tx,&anonymous.session,SessionGrant::full(&user,now),now).await.unwrap();
        tx.commit().await.unwrap();(service,session)
    }
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_schedule_resume_and_revocation(){
    let f=Fixture::new().await;
    let enabled=service::save_schedule(&f.user,true).await.unwrap();let deadline=enabled.accounts[0].next_run_at;
    assert_eq!(service::save_schedule(&f.user,true).await.unwrap().accounts[0].next_run_at,deadline);
    let runtime=service::T212SyncRuntime::start();
    tokio::time::timeout(std::time::Duration::from_secs(5),async{loop{if f.stub.0.lock().unwrap().posts==1{break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}}).await.unwrap();
    runtime.shutdown().await;
    let active=service::status(&f.user).await.unwrap();assert_eq!(active.runs.len(),1);assert_eq!(active.runs[0].report_id.as_deref(),Some("9007199254740993"));
    let coalesced=service::request_run(&f.user,false).await.unwrap();assert_eq!(coalesced.run.id,active.runs[0].id);
    assert!(service::request_run(&f.user,true).await.unwrap_err().downcast_ref::<service::T212SyncError>().is_some());
    let paused=service::save_schedule(&f.user,false).await.unwrap();assert!(!paused.accounts[0].schedule_enabled);
    for _ in 0..4{f.ready().await;f.step().await;}
    let completed=service::status(&f.user).await.unwrap();assert_eq!(completed.runs[0].status,"succeeded");assert_eq!(completed.runs[0].inserted_count,1);assert_eq!(completed.runs[0].skipped_count,1);
    assert!(completed.accounts[0].backfill_complete);assert_eq!(completed.accounts[0].synced_through,Some(completed.runs[0].range_to));
    assert_eq!(f.db.query_one("SELECT count(*) FROM interest WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap().get::<_,i64>(0),1);
    assert_eq!(f.stub.0.lock().unwrap().posts,1);
    // A complete run uses the seven-day overlap and manual requests do not move
    // the paused/daily deadline.
    let incremental=service::request_run(&f.user,false).await.unwrap();assert_eq!(incremental.run.range_from,completed.runs[0].range_to-TimeDelta::days(7));
    let committed=completed.accounts[0].synced_through;
    f.db.execute("UPDATE auth_users SET status='disabled' WHERE id=$1",&[&f.user.as_str()]).await.unwrap();
    f.ready().await;
    let(_stop,rx)=tokio::sync::watch::channel(false);assert!(service::run_test_session(&f.user,&f.account,rx).await.is_err());
    assert_eq!(f.stub.0.lock().unwrap().posts,1);
    assert_eq!(service::status(&f.user).await.unwrap().accounts[0].synced_through,committed);
    f.cleanup().await;

    let f=Fixture::new().await;f.stub.0.lock().unwrap().lost_ack=true;
    let run=service::request_run(&f.user,false).await.unwrap();f.step().await;
    assert_eq!(service::status(&f.user).await.unwrap().runs[0].error_code.as_deref(),Some("request_unconfirmed"));
    f.ready().await;f.step().await;
    assert_eq!(service::status(&f.user).await.unwrap().runs[0].report_id.as_deref(),Some("9007199254740993"));assert_eq!(f.stub.0.lock().unwrap().posts,1);
    f.db.execute("UPDATE t212_sync_run SET report_requested_at=clock_timestamp()-interval '25 hours',next_attempt_at='epoch' WHERE user_id=$1 AND id=$2",&[&f.user.as_str(),&run.run.id]).await.unwrap();
    f.ready().await;f.step().await;
    let expired=service::status(&f.user).await.unwrap();assert_eq!(expired.runs[0].status,"failed");assert_eq!(expired.runs[0].error_code.as_deref(),Some("report_expired"));assert!(expired.accounts[0].synced_through.is_none());
    f.cleanup().await;

    let f=Fixture::new().await;
    f.db.execute("UPDATE t212_sync_account SET backfill_cursor='epoch' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
    let run=service::request_run(&f.user,true).await.unwrap();
    let end=chrono::DateTime::from_timestamp(730*86400+1,0).unwrap();
    f.db.execute("UPDATE t212_sync_run SET range_to=$3 WHERE user_id=$1 AND id=$2",&[&f.user.as_str(),&run.run.id,&end]).await.unwrap();
    f.stub.0.lock().unwrap().csv=b"Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate\nInterest on cash,1971-01-01T00:00:00Z,,,,,1,EUR,boundary,0,1\n".to_vec();
    for _ in 0..4{f.ready().await;f.step().await;}
    let first=service::status(&f.user).await.unwrap();assert_eq!(first.runs[0].cursor.timestamp(),365*86400);assert!(!first.accounts[0].backfill_complete);
    assert_eq!(f.db.query_one("SELECT count(*) FROM interest",&[]).await.unwrap().get::<_,i64>(0),0);
    for _ in 0..4{f.ready().await;f.step().await;}
    let complete=service::status(&f.user).await.unwrap();assert_eq!(complete.runs[0].status,"succeeded");assert_eq!(complete.accounts[0].synced_through,Some(end));
    assert_eq!(f.db.query_one("SELECT count(*) FROM interest",&[]).await.unwrap().get::<_,i64>(0),1);assert_eq!(f.stub.0.lock().unwrap().posts,3);
    f.cleanup().await;

    let f=Fixture::new().await;f.stub.0.lock().unwrap().reject_range=true;
    service::request_run(&f.user,true).await.unwrap();
    for _ in 0..20{f.ready().await;f.step().await;if service::status(&f.user).await.unwrap().runs[0].status=="failed"{break;}}
    let rejected=service::status(&f.user).await.unwrap();assert_eq!(rejected.runs[0].error_code.as_deref(),Some("report_range_rejected"));assert_eq!(rejected.runs[0].cursor.timestamp(),0);assert!(rejected.accounts[0].synced_through.is_none());
    f.cleanup().await;

    let f=Fixture::new().await;f.stub.0.lock().unwrap().rate_once=true;
    service::request_run(&f.user,false).await.unwrap();f.step().await;
    let waiting=service::status(&f.user).await.unwrap();assert_eq!(waiting.runs[0].status,"waiting");assert_eq!(waiting.runs[0].error_code.as_deref(),Some("rate_limited"));assert!(waiting.runs[0].next_attempt_at-Utc::now()>TimeDelta::seconds(118));assert_eq!(f.stub.0.lock().unwrap().posts,1);
    f.cleanup().await;
}


async fn browser_request(router:Router,session:&crate::services::auth::session_store::IssuedSession,method:&str,path:&str,body:Value,csrf:bool)->Response {
    use tower::ServiceExt;
    let mut request=axum::http::Request::builder().method(method).uri(path)
        .header("cookie",format!("doughbox_session={}",session.raw_cookie));
    if method!="GET" {request=request.header("origin","http://localhost:5173").header("content-type","application/json");if csrf {request=request.header("x-csrf-token",&session.session.csrf_token);}}
    router.oneshot(request.body(if method=="GET"{axum::body::Body::empty()}else{axum::body::Body::from(body.to_string())}).unwrap()).await.unwrap()
}
async fn response_json(response:Response)->Value {serde_json::from_slice(&axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap()).unwrap()}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_http_security_and_management(){
    use crate::database::models::auth::{CredentialKind,ApiTokenScope};
    use tower::ServiceExt;
    let mut f=Fixture::new().await;f.five_events().await;
    let(auth,session)=f.browser_session().await;
    let router=crate::api::routes::create_router(auth.clone()).unwrap();
    let status=browser_request(router.clone(),&session,"GET","/api/trading/sync",json!(null),false).await;
    assert_eq!(status.status(),StatusCode::OK);assert!(response_json(status).await["available"].as_bool().unwrap());
    {let state=f.stub.0.lock().unwrap();assert_eq!((state.posts,state.reads),(0,0));}
    let denied=browser_request(router.clone(),&session,"PUT","/api/trading/sync/scheduler",json!({"enabled":true}),false).await;
    assert_eq!(denied.status(),StatusCode::FORBIDDEN);
    let user=auth::by_id(&f.user,&f.db).await.unwrap().unwrap();
    let token=auth.create_api_token(&user.current(CredentialKind::BrowserSession).unwrap(),&session.session,"Fixture read".into(),ApiTokenScope::Read).await.unwrap();
    let denied=router.clone().oneshot(axum::http::Request::builder().uri("/api/trading/sync").header("authorization",format!("Bearer {}",token.token)).body(axum::body::Body::empty()).unwrap()).await.unwrap();
    assert_eq!(denied.status(),StatusCode::FORBIDDEN);
    let missing=browser_request(router.clone(),&session,"GET","/api/trading/sync/reviews?batch_id=foreign",json!(null),false).await;assert_eq!(missing.status(),StatusCode::NOT_FOUND);
    let foreign_owner=UserId::new();let foreign_account=uuid::Uuid::new_v4().to_string();
    f.db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'foreign-owner','fixture','fixture','member','active')",&[&foreign_owner.as_str()]).await.unwrap();
    f.db.execute("INSERT INTO t212_sync_account(user_id,id,broker_account_id,currency) VALUES($1,$2,'foreign-account','EUR')",&[&foreign_owner.as_str(),&foreign_account]).await.unwrap();
    f.db.execute("INSERT INTO t212_import_batch(user_id,id,account_id,source,content_hash) VALUES($1,'owned-by-foreign-user',$2,'statement_csv','foreign-digest')",&[&foreign_owner.as_str(),&foreign_account]).await.unwrap();
    let foreign=browser_request(router.clone(),&session,"GET","/api/trading/sync/reviews?batch_id=owned-by-foreign-user",json!(null),false).await;
    assert_eq!(foreign.status(),StatusCode::NOT_FOUND);
    let csv=String::from_utf8(f.stub.0.lock().unwrap().csv.clone()).unwrap();
    let multipart=format!("--fixture\r\nContent-Disposition: form-data; name=\"trading212_account_id\"\r\n\r\n{foreign_account}\r\n--fixture\r\nContent-Disposition: form-data; name=\"files\"; filename=\"statement.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--fixture--\r\n");
    let foreign=router.clone().oneshot(axum::http::Request::builder().method("POST").uri("/api/statement-imports").header("cookie",format!("doughbox_session={}",session.raw_cookie)).header("origin","http://localhost:5173").header("x-csrf-token",&session.session.csrf_token).header("content-type","multipart/form-data; boundary=fixture").body(axum::body::Body::from(multipart)).unwrap()).await.unwrap();
    assert_eq!(foreign.status(),StatusCode::NOT_FOUND);
    assert_eq!(f.db.query_one("SELECT count(*) FROM statement_import_job",&[]).await.unwrap().get::<_,i64>(0),0);
    let queued=browser_request(router.clone(),&session,"POST","/api/trading/sync/runs",json!({}),true).await;assert_eq!(queued.status(),StatusCode::ACCEPTED);
    assert_eq!(f.stub.0.lock().unwrap().posts,0);
    for _ in 0..4 {f.ready().await;f.step().await;}
    let staged=service::status(&f.user).await.unwrap();assert_eq!(staged.review_batches.len(),1);
    assert_eq!((staged.runs[0].inserted_count,staged.runs[0].matched_count,staged.runs[0].review_count),(4,0,1));
    assert!(staged.accounts[0].synced_through.is_none());
    let batch=&staged.review_batches[0].id;
    let review=browser_request(router.clone(),&session,"GET",&format!("/api/trading/sync/reviews?batch_id={batch}"),json!(null),false).await;
    assert_eq!(review.status(),StatusCode::OK);let review=response_json(review).await;
    let row=&review["items"][0];assert_eq!(row["kind"],"dividend");assert_eq!(row["candidates"][0]["ledger_id"],"pre-feature-dividend");
    let decision=json!({"action":"match_existing","batch_id":batch,"row_number":row["row_number"],"version":row["version"],"account_id":f.account,"kind":"dividend","ledger_id":"pre-feature-dividend","accept_recorded_values":false});
    let mut foreign_decision=decision.clone();foreign_decision["account_id"]=json!(foreign_account);
    let foreign=browser_request(router.clone(),&session,"POST","/api/trading/sync/reviews",json!({"decisions":[foreign_decision]}),true).await;
    assert_eq!(foreign.status(),StatusCode::NOT_FOUND);
    let mut invalid=decision.clone();invalid["row_number"]=json!(999);
    let conflict=browser_request(router.clone(),&session,"POST","/api/trading/sync/reviews",json!({"decisions":[decision.clone(),invalid]}),true).await;
    assert_eq!(conflict.status(),StatusCode::CONFLICT);
    assert_eq!(service::status(&f.user).await.unwrap().review_batches[0].review_count,1);
    let applied=browser_request(router.clone(),&session,"POST","/api/trading/sync/reviews",json!({"decisions":[decision.clone()]}),true).await;assert_eq!(applied.status(),StatusCode::OK);
    let replay=browser_request(router.clone(),&session,"POST","/api/trading/sync/reviews",json!({"decisions":[decision]}),true).await;assert_eq!(replay.status(),StatusCode::OK);
    let completed=service::status(&f.user).await.unwrap();assert!(completed.review_batches.is_empty());assert_eq!(completed.runs[0].status,"succeeded");
    assert_eq!((completed.runs[0].inserted_count,completed.runs[0].matched_count,completed.runs[0].review_count),(4,1,0));
    assert_eq!(f.db.query_one("SELECT (SELECT count(*) FROM trade)+(SELECT count(*) FROM dividend)+(SELECT count(*) FROM interest)+(SELECT count(*) FROM fx_conversion)",&[]).await.unwrap().get::<_,i64>(0),5);
    let preserved=f.db.query_one("SELECT created_via,imported_at FROM t212_activity_event WHERE user_id=$1 AND dividend_id='pre-feature-dividend'",&[&f.user.as_str()]).await.unwrap();assert_eq!(preserved.get::<_,String>(0),"legacy");assert!(preserved.get::<_,Option<chrono::DateTime<Utc>>>(1).is_none());
    let start=Utc::now()-TimeDelta::days(20);let end=Utc::now()+TimeDelta::days(1);
    let events=crate::services::events::get_events(&f.user,start,end).await.unwrap();
    let trade=events.iter().find(|event| event.event_type==crate::services::events::EventType::Trade).unwrap();
    assert_eq!(trade.import_source.as_deref(),Some("trading212_api"));assert!(trade.imported_at.is_some());
    let dividend=events.iter().find(|event| event.event_type==crate::services::events::EventType::Dividend).unwrap();
    assert_eq!(dividend.import_source.as_deref(),Some("legacy"));assert!(dividend.imported_at.is_none());
    let page=crate::services::events::get_events_page(&f.user,&crate::services::events::EventPageFilter{from_date:start,until_date:end,isin:None,broker:None,event_type:None,limit:20,offset:0}).await.unwrap();
    assert!(page.iter().any(|event| event.import_source.as_deref()==Some("trading212_api") && event.imported_at.is_some()));
    assert!(page.iter().any(|event| event.event_type==crate::services::events::EventType::Dividend && event.import_source.as_deref()==Some("legacy") && event.imported_at.is_none()));
    f.cleanup().await;
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_parser_session_loss_and_preapply_revocation() {
    for fault in ["session_loss","owner_disable","account_replace"] {
        let mut f=Fixture::new().await;
        let run=service::request_run(&f.user,false).await.unwrap();
        let time=(run.run.range_from+TimeDelta::seconds(1)).to_rfc3339();
        {
            let mut state=f.stub.0.lock().unwrap();
            state.reads=2;
            state.csv=format!("Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate\nInterest on cash,{time},,,,,1.20,USD,paused-parser,0,1\n").into_bytes();
        }
        f.db.execute("INSERT INTO fx_rate(hash,date,rate,currency_from,currency_to) VALUES('parser-usd','1970-01-01',1.2,'EUR','USD')",&[]).await.unwrap();
        f.step().await;f.ready().await;
        let mut blocker=super::super::reconciliation::connect_schema(&f.schema).await;
        let blocked=blocker.transaction().await.unwrap();
        blocked.batch_execute("LOCK TABLE fx_rate IN ACCESS EXCLUSIVE MODE").await.unwrap();
        let user=f.user.clone();let account=f.account.clone();
        let (_stop,rx)=tokio::sync::watch::channel(false);
        let worker=tokio::spawn(async move {service::run_test_session(&user,&account,rx).await});
        let parser_pid=tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {
                if let Some(row)=f.db.query_opt("SELECT pid FROM pg_locks WHERE relation='fx_rate'::regclass AND mode='AccessShareLock' AND NOT granted",&[]).await.unwrap() {
                    break row.get::<_,i32>(0);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("parser must reach blocked FX enrichment");
        if fault=="session_loss" {
            let lock=format!("doughbox:t212-sync:{}:{}",f.user.as_str(),f.account);
            let worker_pid:i32=f.db.query_one("SELECT pid FROM pg_locks WHERE locktype='advisory' AND granted AND objsubid=1 AND classid=((hashtextextended($1,0)>>32)&4294967295)::oid AND objid=(hashtextextended($1,0)&4294967295)::oid",&[&lock]).await.unwrap().get(0);
            assert_ne!(worker_pid,parser_pid);
            assert!(f.db.query_one("SELECT pg_terminate_backend($1)",&[&worker_pid]).await.unwrap().get::<_,bool>(0));
            assert!(tokio::time::timeout(std::time::Duration::from_secs(5),worker).await.unwrap().unwrap().is_err());
            // Completion while FX remains blocked proves cancellation joined
            // the parser rather than detaching preparation to finish later.
            blocked.rollback().await.unwrap();
        } else {
            if fault=="owner_disable" {
                f.db.execute("UPDATE auth_users SET status='disabled' WHERE id=$1",&[&f.user.as_str()]).await.unwrap();
            } else {
                assert!(crate::services::trading::store::execution_lock(&f.user,&f.db).await.unwrap());
                let credentials=provider::encrypt_credentials(&provider::Credentials{api_key:"replacement".into(),api_secret:"fixture".into()}).unwrap();
                let tx=f.db.transaction().await.unwrap();
                crate::services::trading::store::replace_connection_on(&f.user,&tx,Some(("live",&credentials,&json!({"id":"other-account","currency":"EUR"}),&json!([]))),Some("fixture-generation")).await.unwrap();
                tx.commit().await.unwrap();
            }
            blocked.rollback().await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_secs(5),worker).await.unwrap().unwrap().is_err());
        }
        assert_eq!(f.db.query_one("SELECT count(*) FROM interest",&[]).await.unwrap().get::<_,i64>(0),0);
        assert_eq!(f.db.query_one("SELECT count(*) FROM t212_import_batch",&[]).await.unwrap().get::<_,i64>(0),0);
        let state=service::status(&f.user).await.unwrap();
        assert!(state.accounts.iter().all(|account|account.synced_through.is_none()));
        assert!(state.runs.iter().all(|run|run.status!="succeeded"));
        if fault=="session_loss" {
            f.ready().await;f.step().await;
            assert_eq!(f.db.query_one("SELECT amount_eur FROM interest",&[]).await.unwrap().get::<_,rust_decimal::Decimal>(0),rust_decimal_macros::dec!(1));
            assert_eq!(f.stub.0.lock().unwrap().posts,1);
        }
        drop(blocker);f.cleanup().await;
    }
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_failed_reports_do_not_advance_coverage() {
    for failure in ["missing","failed","non_csv","malformed","too_large","forbidden"] {
        let mut f=Fixture::new().await;
        if failure=="malformed" {f.five_events().await;}
        {
            let mut state=f.stub.0.lock().unwrap();state.reads=2;
            match failure {
                "missing"=>state.missing=true,
                "failed"=>state.report_status=Some("Failed"),
                "non_csv"=>state.csv=b"<!doctype html><html>Not a report</html>".to_vec(),
                "malformed"=>state.csv=String::from_utf8(std::mem::take(&mut state.csv)).unwrap().replace("1.20,EUR,cash-1","invalid-amount,EUR,cash-1").into_bytes(),
                "too_large"=>state.csv=vec![b'x';25*1024*1024+1],
                "forbidden"=>state.forbidden=true,
                _=>unreachable!(),
            }
        }
        let run=service::request_run(&f.user,false).await.unwrap();
        f.step().await;
        if failure!="forbidden" {f.ready().await;f.step().await;}
        let status=service::status(&f.user).await.unwrap();let result=&status.runs[0];
        assert_eq!(result.cursor,run.run.cursor,"{failure}");
        assert!(status.accounts[0].synced_through.is_none(),"{failure}");
        assert!(status.available,"report failure must not disconnect trading");
        if failure=="too_large" {
            assert_eq!(result.status,"queued");
        } else {
            assert_eq!(result.status,"failed","{failure}");
            let code=match failure {"missing"=>"report_missing","failed"=>"report_failed","forbidden"=>"missing_scope",_=>"report_import_failed"};
            assert_eq!(result.error_code.as_deref(),Some(code),"{failure}");
        }
        assert_eq!(f.db.query_one("SELECT (SELECT count(*) FROM trade)+(SELECT count(*) FROM interest)+(SELECT count(*) FROM fx_conversion)",&[]).await.unwrap().get::<_,i64>(0),0,"{failure}");
        assert_eq!(f.db.query_one("SELECT count(*) FROM dividend",&[]).await.unwrap().get::<_,i64>(0),if failure=="malformed"{1}else{0},"{failure}");
        assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event",&[]).await.unwrap().get::<_,i64>(0),0,"{failure}");
        assert_eq!(f.db.query_one("SELECT count(*) FROM t212_import_batch",&[]).await.unwrap().get::<_,i64>(0),0,"{failure}");
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_overdue_catchup_late_arrival_and_full_rescan() {
    let f=Fixture::new().await;
    service::request_run(&f.user,false).await.unwrap();
    for _ in 0..4 {f.ready().await;f.step().await;}
    let baseline=service::status(&f.user).await.unwrap();
    assert_eq!(baseline.runs[0].status,"succeeded");
    let covered=baseline.accounts[0].synced_through.unwrap();
    let late=(covered-TimeDelta::days(3)).to_rfc3339();
    let old=(covered-TimeDelta::days(10)).to_rfc3339();
    f.stub.0.lock().unwrap().csv=format!("Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate\nInterest on cash,{late},,,,,1.40,EUR,late-arrival,0,1\nInterest on cash,{old},,,,,1.80,EUR,older-missing,0,1\n").into_bytes();
    service::save_schedule(&f.user,true).await.unwrap();
    f.db.execute("UPDATE t212_sync_account SET next_run_at=clock_timestamp()-interval '72 hours' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
    f.ready().await;
    let runtime=service::T212SyncRuntime::start();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {if f.stub.0.lock().unwrap().posts==2 {break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
    }).await.unwrap();
    runtime.shutdown().await;
    let caught_up=service::status(&f.user).await.unwrap();
    assert_eq!(caught_up.runs.len(),2);
    assert_eq!(caught_up.runs[0].range_from,covered-TimeDelta::days(7));
    let deadline=caught_up.accounts[0].next_run_at.unwrap();
    assert!((deadline-caught_up.runs[0].created_at-TimeDelta::hours(24)).abs()<TimeDelta::seconds(5));
    for _ in 0..4 {f.ready().await;f.step().await;}
    assert_eq!(f.db.query_one("SELECT sum(amount) FROM interest",&[]).await.unwrap().get::<_,rust_decimal::Decimal>(0),rust_decimal_macros::dec!(2.6));
    let full=service::request_run(&f.user,true).await.unwrap();
    assert_eq!(full.run.range_from.timestamp(),0);
    let max_steps=((full.run.range_to-full.run.range_from).num_days()/365+1)*2+3;
    for _ in 0..max_steps {
        f.ready().await;f.step().await;
        if service::status(&f.user).await.unwrap().runs[0].status=="succeeded" {break;}
    }
    let complete=service::status(&f.user).await.unwrap();
    assert_eq!(complete.runs[0].status,"succeeded");
    assert_eq!((complete.runs[0].inserted_count,complete.runs[0].matched_count),(1,1));
    assert_eq!(complete.accounts[0].next_run_at,Some(deadline));
    let totals=f.db.query_one("SELECT count(*),sum(amount) FROM interest",&[]).await.unwrap();
    assert_eq!(totals.get::<_,i64>(0),3);
    assert_eq!(totals.get::<_,rust_decimal::Decimal>(1),rust_decimal_macros::dec!(4.4));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires an isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn demo_connection_syncs_its_own_activity() {
    let f=Fixture::new().await;
    f.db.execute("UPDATE trading_connection SET environment='demo' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
    f.db.execute("UPDATE t212_sync_account SET environment='demo' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
    let status=service::status(&f.user).await.unwrap();
    assert!(status.available, "{:?}", status.unavailable_reason);
    assert_eq!(status.current_account_id.as_deref(), Some(f.account.as_str()));
    assert_eq!(status.accounts.iter().find(|account| account.id==f.account).unwrap().environment, "demo");
    service::request_run(&f.user, false).await.unwrap();
    f.step().await;
    let environments:Vec<String>=f.db.query("SELECT DISTINCT environment FROM trading_rate_limit",&[]).await.unwrap().into_iter().map(|row| row.get(0)).collect();
    assert_eq!(environments, vec!["demo".to_string()]);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires an isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn refused_csv_export_uses_authorized_history_access() {
    for lost_ack in [false,true] {
        let f=Fixture::new().await;
        f.db.execute("UPDATE trading_connection SET environment='demo' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
        f.db.execute("UPDATE t212_sync_account SET environment='demo' WHERE user_id=$1",&[&f.user.as_str()]).await.unwrap();
        let run=service::request_run(&f.user,false).await.unwrap();
        {
            let mut state=f.stub.0.lock().unwrap();
            state.lost_ack=lost_ack;
            state.forbidden=!lost_ack;
            state.forbidden_body=Some(r#"{"type":"/api-errors/not-available-in-demo-account"}"#.to_owned());
            state.history_fill=Some((run.run.range_from+TimeDelta::seconds(1)).to_rfc3339());
        }
        f.step().await;
        if lost_ack {
            assert_eq!(service::status(&f.user).await.unwrap().runs[0].error_code.as_deref(),Some("request_unconfirmed"));
            f.stub.0.lock().unwrap().reports_forbidden=true;
            f.ready().await;f.step().await;
        }
        for phase in ["history_orders","history_dividends","history_transactions"] {
            let status=service::status(&f.user).await.unwrap();
            assert_eq!(status.runs[0].phase,phase);
            assert_eq!(status.runs[0].cursor,run.run.cursor);
            assert!(status.accounts[0].synced_through.is_none());
            f.ready().await;f.step().await;
        }
        let status=service::status(&f.user).await.unwrap();
        assert_eq!(status.runs[0].status,"succeeded");
        assert_eq!(status.runs[0].cursor,run.run.range_to);
        assert_eq!(status.accounts[0].synced_through,Some(run.run.range_to));
        assert_eq!(status.accounts[0].environment,"demo");
        assert_eq!(f.stub.0.lock().unwrap().posts,1);
        assert_eq!(f.stub.0.lock().unwrap().reads,usize::from(lost_ack));
        assert_eq!(f.db.query_one("SELECT count(*) FROM trade",&[]).await.unwrap().get::<_,i64>(0),1);
        let environments:Vec<String>=f.db.query("SELECT DISTINCT environment FROM trading_rate_limit",&[]).await.unwrap().into_iter().map(|row|row.get(0)).collect();
        assert_eq!(environments,vec!["demo".to_string()]);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn history_preparation_cancels_blocked_fx_before_releasing_session() {
    for fault in ["shutdown","session_loss"] {
        let f=Fixture::new().await;
        let run=service::request_run(&f.user,false).await.unwrap();
        {
            let mut state=f.stub.0.lock().unwrap();
            state.forbidden=true;
            state.forbidden_body=Some(r#"{"type":"/api-errors/not-available-in-demo-account"}"#.to_owned());
            state.history_transaction=Some(json!({"dateTime":(run.run.range_from+TimeDelta::seconds(1)).to_rfc3339(),"type":"INTEREST_ON_FREE_CASH","amount":1.2,"currency":"USD","reference":"blocked-history-interest"}));
        }
        f.db.execute("INSERT INTO fx_rate(hash,date,rate,currency_from,currency_to) VALUES('history-parser-usd','1970-01-01',1.2,'EUR','USD')",&[]).await.unwrap();
        for _ in 0..3 {f.ready().await;f.step().await;}
        assert_eq!(service::status(&f.user).await.unwrap().runs[0].phase,"history_transactions");
        f.ready().await;
        let mut blocker=super::super::reconciliation::connect_schema(&f.schema).await;
        let blocked=blocker.transaction().await.unwrap();
        blocked.batch_execute("LOCK TABLE fx_rate IN ACCESS EXCLUSIVE MODE").await.unwrap();
        let user=f.user.clone();let account=f.account.clone();
        let (stop,rx)=tokio::sync::watch::channel(false);
        let worker=tokio::spawn(async move {service::run_test_session(&user,&account,rx).await});
        let parser_pid=tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {
                if let Some(row)=f.db.query_opt("SELECT pid FROM pg_locks WHERE relation='fx_rate'::regclass AND mode='AccessShareLock' AND NOT granted",&[]).await.unwrap() {break row.get::<_,i32>(0);}
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("history parser must reach blocked FX enrichment");
        if fault=="shutdown" {
            stop.send(true).unwrap();
        } else {
            let lock=format!("doughbox:t212-sync:{}:{}",f.user.as_str(),f.account);
            let worker_pid:i32=f.db.query_one("SELECT pid FROM pg_locks WHERE locktype='advisory' AND granted AND objsubid=1 AND classid=((hashtextextended($1,0)>>32)&4294967295)::oid AND objid=(hashtextextended($1,0)&4294967295)::oid",&[&lock]).await.unwrap().get(0);
            assert_ne!(worker_pid,parser_pid);
            assert!(f.db.query_one("SELECT pg_terminate_backend($1)",&[&worker_pid]).await.unwrap().get::<_,bool>(0));
        }
        let result=tokio::time::timeout(std::time::Duration::from_secs(5),worker).await.expect("cancellation must join history preparation while FX remains blocked").unwrap();
        assert_eq!(result.is_err(),fault=="session_loss");
        let state=service::status(&f.user).await.unwrap();
        assert_eq!(state.runs[0].cursor,run.run.cursor);
        assert_eq!(state.runs[0].phase,"history_transactions");
        assert!(state.accounts[0].synced_through.is_none());
        assert_eq!(f.db.query_one("SELECT count(*) FROM interest",&[]).await.unwrap().get::<_,i64>(0),0);
        assert_eq!(f.db.query_one("SELECT count(*) FROM t212_import_batch",&[]).await.unwrap().get::<_,i64>(0),0);
        blocked.rollback().await.unwrap();
        f.ready().await;f.step().await;
        let state=service::status(&f.user).await.unwrap();
        assert_eq!(state.runs[0].status,"succeeded");
        assert_eq!(state.accounts[0].synced_through,Some(run.run.range_to));
        assert_eq!(f.db.query_one("SELECT amount_eur FROM interest",&[]).await.unwrap().get::<_,rust_decimal::Decimal>(0),rust_decimal_macros::dec!(1));
        drop(blocker);f.cleanup().await;
    }
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn lost_ack_missing_named_history_scope_remains_terminal() {
    let f=Fixture::new().await;
    f.stub.0.lock().unwrap().lost_ack=true;
    let run=service::request_run(&f.user,false).await.unwrap();
    f.step().await;
    f.stub.0.lock().unwrap().reports_forbidden=true;
    f.ready().await;f.step().await;
    let state=service::status(&f.user).await.unwrap();
    assert_eq!(state.runs[0].status,"failed");
    assert_eq!(state.runs[0].error_code.as_deref(),Some("missing_scope"));
    assert_eq!(state.runs[0].cursor,run.run.cursor);
    assert!(state.accounts[0].synced_through.is_none());
    assert_eq!(f.stub.0.lock().unwrap().posts,1);
    f.cleanup().await;
}
