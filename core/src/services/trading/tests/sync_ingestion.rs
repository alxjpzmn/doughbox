use super::*;
use crate::services::importers::trading212::{apply_trading212_batch, prepare_trading212_csv};
use crate::services::trading::sync::{T212ImportContext, T212ImportSource};
use rust_decimal_macros::dec;

struct ImportEnv(Option<std::ffi::OsString>);
impl Drop for ImportEnv {fn drop(&mut self){match self.0.take(){Some(v)=>std::env::set_var("POSTGRES_URL",v),None=>std::env::remove_var("POSTGRES_URL")}}}
struct IngestionFixture { db: tokio_postgres::Client, schema: String, owner: UserId, account: String, _env:ImportEnv }
impl IngestionFixture {
    async fn new() -> Self {
        let url=std::env::var("TEST_POSTGRES_URL").expect("isolated TEST_POSTGRES_URL required");
        let (mut db,connection)=tokio_postgres::connect(&url,NoTls).await.unwrap();tokio::spawn(connection);
        let schema=format!("t212_ingestion_{}",uuid::Uuid::new_v4().simple());
        db.batch_execute(&format!("CREATE SCHEMA {schema};SET search_path TO {schema}")).await.unwrap();
        refinery::embed_migrations!("migrations");migrations::runner().run_async(&mut db).await.unwrap();
        let owner=UserId::new();
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'ingestion-owner','fixture','fixture','member','active')",&[&owner.as_str()]).await.unwrap();
        let account=uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO t212_sync_account(user_id,id,broker_account_id,currency) VALUES($1,$2,'broker-1','EUR')",&[&owner.as_str(),&account]).await.unwrap();
        db.execute("INSERT INTO trading_connection(user_id,generation,environment,credentials,account) VALUES($1,'generation','live','encrypted-fixture',$2)",&[&owner.as_str(),&json!({"id":"broker-1","currency":"EUR"})]).await.unwrap();
        db.batch_execute("INSERT INTO fx_rate(hash,date,rate,currency_from,currency_to) VALUES('fixture-usd','2026-01-01',1.2,'EUR','USD'),('fixture-eur','2026-01-01',1,'EUR','EUR')").await.unwrap();
        let mut scoped=reqwest::Url::parse(&url).unwrap();scoped.query_pairs_mut().append_pair("options",&format!("-csearch_path={schema}"));
        let env=ImportEnv(std::env::var_os("POSTGRES_URL"));std::env::set_var("POSTGRES_URL",scoped.to_string().replace('+',"%20"));
        Self {db,schema,owner,account,_env:env}
    }
    fn manual(&self)->T212ImportContext { T212ImportContext{source:T212ImportSource::StatementCsv,account_id:Some(self.account.clone()),sync_run_id:None,statement_job_id:None} }
    async fn api(&self)->T212ImportContext {
        let id=uuid::Uuid::new_v4().to_string();
        self.db.execute("INSERT INTO t212_sync_run(user_id,id,account_id,connection_generation,trigger,range_from,range_to,cursor,report_from,report_to) VALUES($1,$2,$3,'generation','sync_now','2026-10-01Z','2026-11-01Z','2026-10-01Z','2026-10-01Z','2026-11-01Z')",&[&self.owner.as_str(),&id,&self.account]).await.unwrap();
        T212ImportContext{source:T212ImportSource::Trading212Api,account_id:Some(self.account.clone()),sync_run_id:Some(id),statement_job_id:None}
    }
    async fn apply(&mut self,csv:&[u8],context:&T212ImportContext)->crate::services::trading::sync::T212ImportOutcome {
        let prepared=prepare_trading212_csv(&self.owner,csv).await.unwrap();
        apply_trading212_batch(&self.owner,&mut self.db,prepared,context).await.unwrap()
    }
    async fn count(&self,table:&str)->i64 {self.db.query_one(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"),&[&self.owner.as_str()]).await.unwrap().get(0)}
    async fn cleanup(self){self.db.batch_execute(&format!("SET search_path TO public;DROP SCHEMA {} CASCADE",self.schema)).await.unwrap();}
}
const HEADER:&str="Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate,Withholding tax,Currency (Withholding tax),Notes\n";
fn csv(rows:&str)->Vec<u8>{format!("{HEADER}{rows}").into_bytes()}
fn report()->Vec<u8>{csv("Market buy,2026-10-15 12:00:00,US0378331005,2,10,EUR,20,EUR,order-1,0,1,0,EUR,\nDividend (Ordinary),2026-10-16 12:00:00,US0378331005,2,3,EUR,5,EUR,dividend-1,0,1,1,EUR,\nInterest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,cash-1,0,1,0,EUR,\nLending interest,2026-10-18 12:00:00,,,,,0.30,EUR,loan-1,0,1,0,EUR,\nCurrency conversion,2026-10-19 12:00:00,,,,,9,EUR,fx-1,0,1,0,EUR,10 USD -> 9 EUR\nDeposit,2026-10-20 12:00:00,,,,,100,EUR,deposit,0,1,0,EUR,\nWithdrawal,2026-10-20 12:00:00,,,,,20,EUR,withdrawal,0,1,0,EUR,\n")}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_import_orders_and_legacy_history(){
    let mut f=IngestionFixture::new().await;
    let api=f.api().await;
    let first=f.apply(&report(),&api).await;
    assert_eq!((first.inserted_count,first.matched_count,first.review_count,first.skipped_count),(5,0,0,2));
    assert_eq!((f.count("trade").await,f.count("dividend").await,f.count("interest").await,f.count("fx_conversion").await),(1,1,2,1));
    let manual=f.manual();let replay=f.apply(&report(),&manual).await;
    assert_eq!((replay.inserted_count,replay.matched_count,replay.review_count),(0,5,0));
    let next=f.api().await;let repeat=f.apply(&report(),&next).await;assert_eq!(repeat.matched_count,5);
    assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event WHERE user_id=$1 AND created_via='trading212_api' AND imported_at IS NOT NULL",&[&f.owner.as_str()]).await.unwrap().get::<_,i64>(0),5);
    assert_eq!(f.db.query_one("SELECT amount FROM dividend WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(6));
    // Explicit ledger deletion cascades evidence, but not its owner's observation.
    f.db.execute("DELETE FROM trade WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap();
    let restored=f.apply(&report(),&manual).await;assert_eq!((restored.inserted_count,restored.matched_count),(1,4));
    assert_eq!(f.count("trade").await,1);
    f.cleanup().await;

    let mut f=IngestionFixture::new().await;
    let manual=f.manual();let first=f.apply(&report(),&manual).await;assert_eq!(first.inserted_count,5);
    let api=f.api().await;let observed=f.apply(&report(),&api).await;assert_eq!(observed.matched_count,5);
    assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event WHERE user_id=$1 AND created_via='statement_csv'",&[&f.owner.as_str()]).await.unwrap().get::<_,i64>(0),5);
    f.cleanup().await;

    let mut f=IngestionFixture::new().await;
    let prepared=prepare_trading212_csv(&f.owner,&report()).await.unwrap();
    let tx=f.db.transaction().await.unwrap();
    for row in prepared.rows {
        use crate::services::importers::trading212::T212Financial;
        match row.financial {
            financial@T212Financial::Trade(_) => crate::services::importers::trading212::insert_financial(&f.owner,&tx,&crate::services::shared::util::hash_string("Trading212order-1"),&financial).await.unwrap(),
            T212Financial::Dividend(payment) => assert!(crate::database::queries::dividend::add_dividend_to_db(&f.owner,payment,None).await.unwrap()),
            T212Financial::Interest(payment) => assert!(crate::database::queries::interest::add_interest_to_db(&f.owner,payment,None).await.unwrap()),
            T212Financial::Fx(conversion) => crate::database::queries::fx_conversion::add_fx_conversion_to_db(&f.owner,conversion).await.unwrap(),
        }
    }
    tx.commit().await.unwrap();
    let hashes_sql="SELECT 'trade:'||hash AS id FROM trade UNION ALL SELECT 'dividend:'||id FROM dividend UNION ALL SELECT 'interest:'||id FROM interest UNION ALL SELECT 'fx:'||id FROM fx_conversion ORDER BY id";
    let original_ids:Vec<String>=f.db.query(hashes_sql,&[]).await.unwrap().iter().map(|row|row.get(0)).collect();
    let api=f.api().await;let staged=f.apply(&report(),&api).await;
    assert_eq!((staged.inserted_count,staged.matched_count,staged.review_count),(0,1,4));
    let pending=crate::services::trading::sync::reviews::list(&f.owner,&staged.batch_id,None,50).await.unwrap();
    let decisions=pending.items.iter().map(|row|serde_json::from_value(json!({"action":"match_existing","batch_id":row.batch_id,"row_number":row.row_number,"version":row.version,"account_id":f.account,"kind":row.kind,"ledger_id":row.candidates[0].ledger_id,"accept_recorded_values":false})).unwrap()).collect();
    crate::services::trading::sync::reviews::resolve(&f.owner,decisions).await.unwrap();
    assert_eq!((f.count("trade").await,f.count("dividend").await,f.count("interest").await,f.count("fx_conversion").await),(1,1,2,1));
    assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event WHERE user_id=$1 AND created_via='legacy' AND imported_at IS NULL",&[&f.owner.as_str()]).await.unwrap().get::<_,i64>(0),5);
    assert_eq!(f.db.query(hashes_sql,&[]).await.unwrap().iter().map(|row|row.get::<_,String>(0)).collect::<Vec<_>>(),original_ids);
    let unassigned=T212ImportContext{source:T212ImportSource::StatementCsv,account_id:None,sync_run_id:None,statement_job_id:None};
    let extra=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,distinct-occurrence,0,1,0,EUR,\n"),&unassigned).await;
    assert_eq!((extra.inserted_count,extra.review_count),(0,1));
    let pending=crate::services::trading::sync::reviews::list(&f.owner,&extra.batch_id,None,50).await.unwrap();
    let row=&pending.items[0];
    let decision=serde_json::from_value(json!({"action":"import_new","batch_id":row.batch_id,"row_number":row.row_number,"version":row.version,"account_id":f.account})).unwrap();
    crate::services::trading::sync::reviews::resolve(&f.owner,vec![decision]).await.unwrap();assert_eq!(f.count("interest").await,3);
    f.cleanup().await;
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_identity_boundaries_and_concurrency(){
    let mut f=IngestionFixture::new().await;
    let rows="Interest on cash,2026-10-15T12:00:00.123456789Z,,,,,1.2000,EUR,,0,1,0,EUR,\nInterest on cash,2026-10-15T14:00:00.123456+02:00,,,,,1.2,EUR,,0,1,0,EUR,\n";
    let api=f.api().await;let first=f.apply(&csv(rows),&api).await;assert_eq!(first.inserted_count,2);
    let manual=f.manual();let replay=f.apply(&csv(rows),&manual).await;assert_eq!(replay.matched_count,2);assert_eq!(f.count("interest").await,2);
    let bad=csv("Market buy,2026-10-16 12:00:00,US0378331005,2,10,EUR,20,EUR,new-order,0,1,0,EUR,\nInterest on cash,2026-10-17 12:00:00,,,,,not-an-amount,EUR,bad,0,1,0,EUR,\n");
    assert!(prepare_trading212_csv(&f.owner,&bad).await.is_err());assert_eq!(f.count("trade").await,0);
    let prepared=prepare_trading212_csv(&f.owner,&report()).await.unwrap();
    f.db.batch_execute("CREATE FUNCTION reject_t212_test_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture write failure'; END $$; CREATE TRIGGER reject_t212_test_write BEFORE INSERT ON dividend FOR EACH ROW EXECUTE FUNCTION reject_t212_test_write()").await.unwrap();
    assert!(apply_trading212_batch(&f.owner,&mut f.db,prepared,&manual).await.is_err());
    assert_eq!(f.count("trade").await,0);assert_eq!(f.count("interest").await,2);
    assert_eq!(f.count("t212_import_batch").await,2);
    f.cleanup().await;

    let mut f=IngestionFixture::new().await;let manual=f.manual();
    let mut other=crate::services::trading::tests::reconciliation::connect_schema(&f.schema).await;
    let one=prepare_trading212_csv(&f.owner,&report()).await.unwrap();let two=prepare_trading212_csv(&f.owner,&report()).await.unwrap();
    let api=f.api().await;
    let unassigned=T212ImportContext{source:T212ImportSource::StatementCsv,account_id:None,sync_run_id:None,statement_job_id:None};
    let (left,right)=tokio::join!(apply_trading212_batch(&f.owner,&mut f.db,one,&unassigned),apply_trading212_batch(&f.owner,&mut other,two,&api));
    let left=left.unwrap();let right=right.unwrap();assert_eq!(left.inserted_count+right.inserted_count,5);assert_eq!(left.matched_count+right.matched_count+left.review_count+right.review_count,5);
    assert_eq!(f.count("t212_activity_event").await,5);
    let usd=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,USD,usd-distinct,0,1,0,USD,\n"),&manual).await;
    assert_eq!(usd.inserted_count,1);
    assert_eq!(f.db.query_one("SELECT sum(amount_eur) FROM interest WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(2.5));
    drop(other);f.cleanup().await;
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_unassigned_corrections_and_legacy_currency_collisions_require_review() {
    let mut f=IngestionFixture::new().await;
    let manual=f.manual();f.apply(&report(),&manual).await;
    let unassigned=T212ImportContext{source:T212ImportSource::StatementCsv,account_id:None,sync_run_id:None,statement_job_id:None};
    let correction=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,2.20,EUR,cash-1,0,1,0,EUR,\n"),&unassigned).await;
    assert_eq!((correction.inserted_count,correction.review_count),(0,1));
    assert_eq!(f.count("interest").await,2);
    let pending=crate::services::trading::sync::reviews::list(&f.owner,&correction.batch_id,None,50).await.unwrap();
    let row=&pending.items[0];
    assert_eq!(row.candidates.len(),1);
    assert!(row.candidates[0].values_differ);
    let decision=serde_json::from_value(json!({"action":"match_existing","batch_id":row.batch_id,"row_number":row.row_number,"version":row.version,"account_id":f.account,"kind":row.kind,"ledger_id":row.candidates[0].ledger_id,"accept_recorded_values":true})).unwrap();
    crate::services::trading::sync::reviews::resolve(&f.owner,vec![decision]).await.unwrap();
    let overlap=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,2.20,EUR,cash-1,0,1,0,EUR,\nDeposit,2026-10-20 12:00:00,,,,,100,EUR,deposit,0,1,0,EUR,\n"),&manual).await;
    assert_eq!((overlap.inserted_count,overlap.matched_count,overlap.review_count),(0,1,0));
    assert_eq!(f.db.query_one("SELECT amount FROM interest WHERE user_id=$1 AND principal='Cash'",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(1.2));
    f.cleanup().await;

    let mut f=IngestionFixture::new().await;
    let euro=csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,,0,1,0,EUR,\n");
    let mut prepared=prepare_trading212_csv(&f.owner,&euro).await.unwrap();
    if let crate::services::importers::trading212::T212Financial::Interest(payment)=prepared.rows.remove(0).financial {
        crate::database::queries::interest::add_interest_to_db(&f.owner,payment,None).await.unwrap();
    } else {panic!("expected fixture interest");}
    let manual=f.manual();
    let usd=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,USD,usd-distinct,0,1,0,USD,\n"),&manual).await;
    assert_eq!((usd.inserted_count,usd.review_count),(0,1));
    assert_eq!(f.count("interest").await,1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_shared_ids_cardinality_and_owner_account_boundaries() {
    let mut f=IngestionFixture::new().await;
    let executions="Market buy,2026-10-15 12:00:00,US0378331005,1,10,EUR,10,EUR,shared-order,0,1,0,EUR,\nMarket buy,2026-10-15 12:00:00,US0378331005,2,11,EUR,22,EUR,shared-order,0,1,0,EUR,\nInterest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,income-A,0,1,0,EUR,\nInterest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,income-B,0,1,0,EUR,\n";
    let api=f.api().await;let first=f.apply(&csv(executions),&api).await;assert_eq!(first.inserted_count,4);
    let manual=f.manual();assert_eq!(f.apply(&csv(executions),&manual).await.matched_count,4);
    let extended=format!("{executions}Market buy,2026-10-15 12:00:00,US0378331005,1,10,EUR,10,EUR,shared-order,0,1,0,EUR,\nMarket buy,2026-10-15 12:00:00,US0378331005,3,12,EUR,36,EUR,shared-order,0,1,0,EUR,\n");
    let api=f.api().await;let extra=f.apply(&csv(&extended),&api).await;
    assert_eq!((extra.inserted_count,extra.matched_count,extra.review_count),(2,4,0));
    let api=f.api().await;let replay=f.apply(&csv(&extended),&api).await;
    assert_eq!((replay.inserted_count,replay.matched_count,replay.review_count),(0,6,0));
    assert_eq!(f.db.query_one("SELECT sum(units) FROM trade WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(7));
    assert_eq!(f.db.query_one("SELECT sum(amount) FROM interest WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(2.4));
    let disjoint=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,income-C,0,1,0,EUR,\n"),&manual).await;
    assert_eq!((disjoint.inserted_count,disjoint.review_count),(0,1));
    let account=uuid::Uuid::new_v4().to_string();
    f.db.execute("INSERT INTO t212_sync_account(user_id,id,broker_account_id,currency) VALUES($1,$2,'broker-2','EUR')",&[&f.owner.as_str(),&account]).await.unwrap();
    let other_account=T212ImportContext{account_id:Some(account.clone()),..f.manual()};
    assert_eq!(f.apply(&csv(executions),&other_account).await.inserted_count,4);
    let other_owner=UserId::new();let other_id=uuid::Uuid::new_v4().to_string();
    f.db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'second-ingestion-owner','fixture','fixture','member','active')",&[&other_owner.as_str()]).await.unwrap();
    f.db.execute("INSERT INTO t212_sync_account(user_id,id,broker_account_id,currency) VALUES($1,$2,'broker-1','EUR')",&[&other_owner.as_str(),&other_id]).await.unwrap();
    let prepared=prepare_trading212_csv(&other_owner,&csv(executions)).await.unwrap();
    let context=T212ImportContext{source:T212ImportSource::StatementCsv,account_id:Some(other_id.clone()),sync_run_id:None,statement_job_id:None};
    assert_eq!(apply_trading212_batch(&other_owner,&mut f.db,prepared,&context).await.unwrap().inserted_count,4);
    for (owner,account,count) in [(&f.owner,&f.account,6_i64),(&f.owner,&account,4),(&other_owner,&other_id,4)] {
        assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event WHERE user_id=$1 AND account_id=$2",&[&owner.as_str(),account]).await.unwrap().get::<_,i64>(0),count);
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore="Requires isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_digest_assignment_checks_reviewed_identity_aliases() {
    let mut f=IngestionFixture::new().await;
    let unassigned=T212ImportContext{source:T212ImportSource::StatementCsv,account_id:None,sync_run_id:None,statement_job_id:None};
    let unknown=csv("Interest on cash,2026-10-17 12:00:00,,,,,2.20,EUR,,0,1,0,EUR,\n");
    assert_eq!(f.apply(&unknown,&unassigned).await.inserted_count,1);
    let manual=f.manual();
    assert_eq!(f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,1.20,EUR,known-income,0,1,0,EUR,\n"),&manual).await.inserted_count,1);
    let corrected=f.apply(&csv("Interest on cash,2026-10-17 12:00:00,,,,,2.20,EUR,known-income,0,1,0,EUR,\n"),&manual).await;
    assert_eq!(corrected.review_count,1);
    let review=crate::services::trading::sync::reviews::list(&f.owner,&corrected.batch_id,None,50).await.unwrap();
    let row=&review.items[0];let candidate=row.candidates.iter().find(|candidate|candidate.account_id.as_ref()==Some(&f.account)).unwrap();
    let decision=serde_json::from_value(json!({"action":"match_existing","batch_id":row.batch_id,"row_number":row.row_number,"version":row.version,"account_id":f.account,"kind":row.kind,"ledger_id":candidate.ledger_id,"accept_recorded_values":true})).unwrap();
    crate::services::trading::sync::reviews::resolve(&f.owner,vec![decision]).await.unwrap();
    let assignment=f.apply(&unknown,&manual).await;
    assert_eq!((assignment.inserted_count,assignment.matched_count,assignment.review_count),(0,0,1));
    assert_eq!(f.db.query_one("SELECT count(*) FROM t212_activity_event WHERE user_id=$1 AND account_id IS NULL",&[&f.owner.as_str()]).await.unwrap().get::<_,i64>(0),1);
    assert_eq!(f.db.query_one("SELECT sum(amount) FROM interest WHERE user_id=$1",&[&f.owner.as_str()]).await.unwrap().get::<_,rust_decimal::Decimal>(0),dec!(3.4));
    f.cleanup().await;
}
