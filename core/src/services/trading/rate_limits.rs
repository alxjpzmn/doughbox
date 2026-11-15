use std::time::Duration;

use anyhow::{ensure, Context};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::{header::HeaderMap, StatusCode};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};
use tokio_postgres::{Client, Transaction};

use crate::{database::{db_client, models::auth::UserId, queries::auth}, services::auth::AuthError};
use super::provider::Credentials;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionMode { Try, Wait }

#[derive(Clone)]
pub enum BrokerAccess {
    Owner { user_id: UserId, generation: String },
    Agent { user_id: UserId, generation: String, token_hash: String },
    ConnectionSetup { user_id: UserId, expected_generation: String },
    Approved { user_id: UserId, generation: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerRateScope { pub environment: String, pub kind: String, pub id: String }
impl BrokerRateScope {
    pub fn account(environment: &str, id: &str) -> Self { Self {environment:environment.into(),kind:"account".into(),id:id.into()} }
    pub fn key(environment: &str, key_hash: &str) -> Self { Self {environment:environment.into(),kind:"key".into(),id:key_hash.into()} }
}
#[derive(Clone)]
pub struct BrokerRequestContext { pub scope: BrokerRateScope, pub mode: AdmissionMode, pub access: BrokerAccess }

#[derive(Debug)]
pub struct BrokerRateLimited { pub retry_after_seconds: u64 }
impl std::fmt::Display for BrokerRateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f,"Trading 212 request rate limited; retry after {} seconds.",self.retry_after_seconds) }
}
impl std::error::Error for BrokerRateLimited {}

pub fn key_hash(credentials: &Credentials) -> String { blake3::hash(credentials.api_key.as_bytes()).to_hex().to_string() }

pub fn spacing_millis(bucket: &str) -> anyhow::Result<i64> {
    Ok(match bucket {
        "account_summary"|"pending_orders"|"order_history"|"pie_request"=>6000,
        "positions"=>2000,"instruments"=>60000,"limit_order"=>3000,
        "pie_list"|"export_create"=>31000,"export_status"=>61000,
        _=>anyhow::bail!("Unknown Trading 212 request budget"),
    })
}

pub async fn setup_scope(environment: &str, credentials: &Credentials) -> anyhow::Result<BrokerRateScope> {
    let hash=key_hash(credentials);
    let db=db_client().await?;
    Ok(match db.query_opt("SELECT broker_account_id FROM trading_api_key_account WHERE environment=$1 AND key_hash=$2", &[&environment,&hash]).await? {
        Some(row)=>BrokerRateScope::account(environment,&row.get::<_,String>(0)),
        None=>BrokerRateScope::key(environment,&hash),
    })
}

pub async fn connection_scope(environment: &str, account: &Value, credentials: &Credentials) -> anyhow::Result<BrokerRateScope> {
    let id=account.get("id").and_then(Value::as_str).filter(|id|!id.trim().is_empty()).context("Verified Trading 212 account identity is missing. Reconnect the account.")?;
    promote_scope(&BrokerRateScope::key(environment,&key_hash(credentials)),id,&key_hash(credentials)).await
}

/// Alias registration and consumed-deadline migration are one transaction. An
/// already-created budget remains conservative when another key discovers it.
pub async fn promote_scope(scope: &BrokerRateScope, broker_account_id: &str, key_hash: &str) -> anyhow::Result<BrokerRateScope> {
    ensure!(!broker_account_id.trim().is_empty(),"Invalid Trading 212 account identity");
    let mut db=db_client().await?;
    let tx=db.transaction().await?;
    // A provisional claim resolves its alias while holding this same lock. This
    // prevents promotion racing a still-provisional dispatch reservation.
    tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended('doughbox:t212-key:' || $1::text || ':' || $2::text,0))", &[&scope.environment,&key_hash]).await?;
    tx.execute("INSERT INTO trading_api_key_account(environment,key_hash,broker_account_id) VALUES($1,$2,$3)
        ON CONFLICT(environment,key_hash) DO UPDATE SET broker_account_id=EXCLUDED.broker_account_id",
        &[&scope.environment,&key_hash,&broker_account_id]).await?;
    tx.execute("INSERT INTO trading_rate_limit(environment,scope_kind,scope_id,bucket,next_at,blocked_until,interval_millis)
        SELECT environment,'account',$3,bucket,next_at,blocked_until,interval_millis FROM trading_rate_limit
        WHERE environment=$1 AND scope_kind='key' AND scope_id=$2
        ON CONFLICT(environment,scope_kind,scope_id,bucket) DO UPDATE SET
          next_at=GREATEST(trading_rate_limit.next_at,EXCLUDED.next_at),
          blocked_until=GREATEST(trading_rate_limit.blocked_until,EXCLUDED.blocked_until),
          interval_millis=GREATEST(trading_rate_limit.interval_millis,EXCLUDED.interval_millis)",
        &[&scope.environment,&key_hash,&broker_account_id]).await?;
    tx.commit().await?;
    Ok(BrokerRateScope::account(&scope.environment,broker_account_id))
}

async fn authority(tx: &Transaction<'_>, access: &BrokerAccess, read: bool) -> anyhow::Result<()> {
    let (user_id,generation)=match access {
        BrokerAccess::Owner{user_id,generation}|BrokerAccess::Agent{user_id,generation,..}|BrokerAccess::Approved{user_id,generation}=>(user_id,generation),
        BrokerAccess::ConnectionSetup{user_id,expected_generation}=>(user_id,expected_generation),
    };
    let owner=auth::lock_user(user_id,tx).await?;
    let approved=matches!(access,BrokerAccess::Approved{..}) && !read;
    ensure!(approved || owner.fully_enrolled(),AuthError::Unauthenticated);
    let row=tx.query_opt("SELECT * FROM trading_connection WHERE user_id=$1 FOR UPDATE", &[&user_id.as_str()]).await?.context("Trading connection is no longer available")?;
    ensure!(row.get::<_,String>("generation")==*generation,"Trading connection changed. Refresh before continuing.");
    if !matches!(access,BrokerAccess::ConnectionSetup{..}) {
        ensure!(row.get::<_,Option<String>>("credentials").is_some(),"Connect Trading 212 first.");
    }
    if let BrokerAccess::Agent{token_hash,..}=access {
        ensure!(row.get::<_,Option<String>>("agent_token_hash").as_ref()==Some(token_hash)
            && row.get::<_,Option<i64>>("agent_auth_version")==Some(owner.auth_version),"Invalid or revoked trading token.");
    }
    if approved {
        ensure!(tx.query_opt("SELECT 1 FROM trading_proposals WHERE user_id=$1 AND generation=$2 AND status='executing' LIMIT 1", &[&user_id.as_str(),generation]).await?.is_some(),"Approved trading execution is no longer current.");
    }
    Ok(())
}

async fn resolved_scope(tx: &Transaction<'_>, scope: &BrokerRateScope) -> anyhow::Result<BrokerRateScope> {
    if scope.kind!="key" {return Ok(scope.clone());}
    tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended('doughbox:t212-key:' || $1::text || ':' || $2::text,0))", &[&scope.environment,&scope.id]).await?;
    Ok(match tx.query_opt("SELECT broker_account_id FROM trading_api_key_account WHERE environment=$1 AND key_hash=$2", &[&scope.environment,&scope.id]).await? {
        Some(row)=>BrokerRateScope::account(&scope.environment,&row.get::<_,String>(0)),None=>scope.clone(),
    })
}

async fn remaining(tx: &Transaction<'_>, scope: &BrokerRateScope, bucket: &str) -> anyhow::Result<u64> {
    let text:String=tx.query_one("SELECT CASE WHEN GREATEST(next_at,blocked_until)='infinity'::timestamptz THEN '18446744073709551615'
        ELSE GREATEST(1,CEIL(EXTRACT(EPOCH FROM (GREATEST(next_at,blocked_until)-clock_timestamp()))))::text END
        FROM trading_rate_limit WHERE environment=$1 AND scope_kind=$2 AND scope_id=$3 AND bucket=$4",
        &[&scope.environment,&scope.kind,&scope.id,&bucket]).await?.get(0);
    Ok(text.parse::<u64>().context("Invalid broker budget deadline")?.max(1))
}

pub struct BrokerBudget { context: BrokerRequestContext, db: OnceCell<Mutex<Client>> }
impl BrokerBudget {
    pub fn new(context: BrokerRequestContext)->Self {Self {context,db:OnceCell::new()}}
    async fn db(&self)->anyhow::Result<&Mutex<Client>> {self.db.get_or_try_init(||async {Ok::<_,anyhow::Error>(Mutex::new(db_client().await?))}).await}
    pub async fn promote(&mut self,broker_account_id:&str,key_hash:&str)->anyhow::Result<()> {
        self.context.scope=promote_scope(&self.context.scope,broker_account_id,key_hash).await?;Ok(())
    }
    pub async fn authorize(&self,read:bool)->anyhow::Result<()> {
        let mut db=self.db().await?.lock().await;let tx=db.transaction().await?;
        authority(&tx,&self.context.access,read).await?;tx.commit().await?;Ok(())
    }
    pub async fn admit(&self,bucket:&str,read:bool)->anyhow::Result<()> {
        let spacing=spacing_millis(bucket)?;
        loop {
            let retry={
                let mut db=self.db().await?.lock().await;let tx=db.transaction().await?;
                authority(&tx,&self.context.access,read).await?;
                let scope=resolved_scope(&tx,&self.context.scope).await?;
                tx.execute("INSERT INTO trading_rate_limit(environment,scope_kind,scope_id,bucket,interval_millis) VALUES($1,$2,$3,$4,$5)
                    ON CONFLICT(environment,scope_kind,scope_id,bucket) DO UPDATE SET interval_millis=GREATEST(trading_rate_limit.interval_millis,EXCLUDED.interval_millis)",
                    &[&scope.environment,&scope.kind,&scope.id,&bucket,&spacing]).await?;
                let claimed=tx.query_opt("UPDATE trading_rate_limit SET next_at=clock_timestamp()+interval_millis*interval '1 millisecond'
                    WHERE environment=$1 AND scope_kind=$2 AND scope_id=$3 AND bucket=$4 AND GREATEST(next_at,blocked_until)<=clock_timestamp() RETURNING 1",
                    &[&scope.environment,&scope.kind,&scope.id,&bucket]).await?.is_some();
                let retry=if claimed {None} else {Some(remaining(&tx,&scope,bucket).await?)};
                tx.commit().await?;retry
            };
            let Some(seconds)=retry else {return Ok(());};
            if self.context.mode==AdmissionMode::Try {return Err(BrokerRateLimited{retry_after_seconds:seconds}.into());}
            self.authorize(read).await?;
            // Periodic authority checks do not shorten the persisted deadline.
            tokio::time::sleep(Duration::from_secs(seconds.min(3600))).await;
            self.authorize(read).await?;
            // Wakeups never reserve a future slot; compete for a current one.
        }
    }
    pub async fn observe(&self,bucket:&str,status:StatusCode,headers:&HeaderMap)->anyhow::Result<u64> {
        let observation=header_observation(bucket,status,headers,Utc::now())?;
        let mut db=self.db().await?.lock().await;let tx=db.transaction().await?;
        // Observations intentionally do not reauthorize. A stale read still
        // consumed broker capacity and must retain its response's cooldown.
        let scope=resolved_scope(&tx,&self.context.scope).await?;
        tx.execute("INSERT INTO trading_rate_limit(environment,scope_kind,scope_id,bucket,interval_millis,blocked_until)
            VALUES($1,$2,$3,$4,$5,CASE WHEN $7 THEN 'infinity'::timestamptz ELSE COALESCE($6,'epoch'::timestamptz) END)
            ON CONFLICT(environment,scope_kind,scope_id,bucket) DO UPDATE SET
              blocked_until=GREATEST(trading_rate_limit.blocked_until,EXCLUDED.blocked_until),
              interval_millis=GREATEST(trading_rate_limit.interval_millis,EXCLUDED.interval_millis),
              next_at=GREATEST(trading_rate_limit.next_at,
                CASE WHEN EXCLUDED.interval_millis>trading_rate_limit.interval_millis THEN clock_timestamp()+EXCLUDED.interval_millis*interval '1 millisecond' ELSE 'epoch'::timestamptz END)",
            &[&scope.environment,&scope.kind,&scope.id,&bucket,&observation.interval_millis,&observation.until,&observation.infinite]).await?;
        let retry=remaining(&tx,&scope,bucket).await?;tx.commit().await?;Ok(retry)
    }
}

struct HeaderObservation { until:Option<DateTime<Utc>>,infinite:bool,interval_millis:i64 }
fn header_text<'a>(headers:&'a HeaderMap,name:&str)->Option<&'a str> {headers.get(name).and_then(|v|v.to_str().ok())}
fn http_date(raw:&str)->Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw).ok().map(|d|d.with_timezone(&Utc))
        .or_else(||chrono::NaiveDateTime::parse_from_str(raw,"%A, %d-%b-%y %H:%M:%S GMT").ok().map(|d|d.and_utc()))
        .or_else(||chrono::NaiveDateTime::parse_from_str(raw,"%a %b %e %H:%M:%S %Y").ok().map(|d|d.and_utc()))
}
fn header_observation(bucket:&str,status:StatusCode,headers:&HeaderMap,now:DateTime<Utc>)->anyhow::Result<HeaderObservation> {
    let spacing=spacing_millis(bucket)?;
    let mut result=HeaderObservation{until:None,infinite:false,interval_millis:spacing};
    if let (Some(limit),Some(period))=(header_text(headers,"x-ratelimit-limit"),header_text(headers,"x-ratelimit-period")) {
        if let (Ok(limit),Ok(period))=(limit.parse::<Decimal>(),period.parse::<Decimal>()) {
            if limit>Decimal::ZERO && period>Decimal::ZERO {
                match period.checked_div(limit).and_then(|p|p.checked_mul(Decimal::from(1000))).map(|p|p.ceil()).and_then(|p|p.to_i64()) {
                    Some(millis) => result.interval_millis=result.interval_millis.max(millis),
                    None => result.infinite=true,
                }
            }
        }
    }
    if let Some(raw)=header_text(headers,"retry-after") {
        if let Ok(seconds)=raw.parse::<u64>() {
            result.until=i64::try_from(seconds).ok().and_then(TimeDelta::try_seconds).and_then(|d|now.checked_add_signed(d));
            result.infinite |= result.until.is_none();
        } else {result.until=http_date(raw);}
    }
    if header_text(headers,"x-ratelimit-remaining").is_some_and(|v|v.parse::<Decimal>().is_ok_and(|v|v==Decimal::ZERO)) {
        if let Some(reset)=header_text(headers,"x-ratelimit-reset").and_then(|v|v.parse::<i64>().ok()).and_then(|v|DateTime::from_timestamp(v,0)) {
            result.until=Some(result.until.map_or(reset,|until|until.max(reset)));
        }
    }
    if status==StatusCode::TOO_MANY_REQUESTS && !result.infinite && result.until.is_none_or(|until|until<=now) {
        result.until=now.checked_add_signed(TimeDelta::milliseconds(result.interval_millis));
    }
    Ok(result)
}

/// Shared only with transport-only test fixtures; production observes SQL state.
#[cfg(test)]
pub(crate) fn response_retry_after(bucket:&str,status:StatusCode,headers:&HeaderMap)->u64 {
    let now=Utc::now();
    match header_observation(bucket,status,headers,now) {
        Ok(value) if value.infinite=>u64::MAX,
        Ok(value)=>value.until.map(|until|until.signed_duration_since(now).num_milliseconds().max(0) as u64)
            .unwrap_or(0).saturating_add(999).checked_div(1000).unwrap_or(1).max((value.interval_millis as u64).div_ceil(1000)).max(1),
        Err(_)=>1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn broker_headers_never_shorten_delay_or_spacing() {
        let now=DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let mut headers=HeaderMap::new();headers.insert("retry-after","120".parse().unwrap());
        headers.insert("x-ratelimit-remaining","0".parse().unwrap());headers.insert("x-ratelimit-reset",(now.timestamp()+180).to_string().parse().unwrap());
        headers.insert("x-ratelimit-limit","2".parse().unwrap());headers.insert("x-ratelimit-period","20".parse().unwrap());
        let value=header_observation("pie_request",StatusCode::TOO_MANY_REQUESTS,&headers,now).unwrap();
        assert_eq!(value.until,Some(now+TimeDelta::seconds(180)));assert_eq!(value.interval_millis,10000);
        headers.insert("retry-after",(now+TimeDelta::seconds(300)).to_rfc2822().parse().unwrap());
        assert_eq!(header_observation("pie_request",StatusCode::BAD_REQUEST,&headers,now).unwrap().until,Some(now+TimeDelta::seconds(300)));
        headers.insert("retry-after","invalid".parse().unwrap());headers.remove("x-ratelimit-reset");
        assert_eq!(header_observation("export_status",StatusCode::TOO_MANY_REQUESTS,&headers,now).unwrap().until,Some(now+TimeDelta::seconds(61)));
        headers.insert("retry-after",u64::MAX.to_string().parse().unwrap());
        assert!(header_observation("positions",StatusCode::TOO_MANY_REQUESTS,&headers,now).unwrap().infinite);
    }
}
