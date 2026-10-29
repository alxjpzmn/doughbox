use std::{panic::AssertUnwindSafe, path::Path, time::Duration};

use futures::FutureExt;
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{interval, MissedTickBehavior},
};
use tokio_postgres::Client;

use crate::{
    database::{
        db_client, db_connection,
        models::statement_import::{StatementImportFile, StatementImportsStatus},
        queries::statement_import::{self, ClaimedStatementImport},
    },
    services::parsers::parse_file_for_upload,
};

const PARTIAL_IMPORT_WARNING: &str = "Some records may already have been imported; review them before uploading the file again.";
const PANIC_ERROR: &str = "Statement importer stopped unexpectedly while reading this file. Check the statement format. Some records may already have been imported; review them before uploading the file again.";

pub async fn status() -> anyhow::Result<StatementImportsStatus> {
    statement_import::list(&db_client().await?).await
}

pub async fn enqueue(files: &[StatementImportFile]) -> anyhow::Result<StatementImportsStatus> {
    statement_import::enqueue(&mut db_client().await?, files).await
}

/// The API owns this worker, not the browser or an individual upload request.
pub struct StatementImportRuntime {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl StatementImportRuntime {
    pub fn start() -> Self {
        let (stop, receiver) = watch::channel(false);
        Self { stop, task: tokio::spawn(worker(receiver)) }
    }

    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        // A spawn_blocking parser cannot be aborted safely. Wait for cooperative
        // cancellation before releasing a healthy advisory-lock session, rather
        // than detaching a parser which could still be writing records.
        let _ = self.task.await;
    }
}

async fn worker(mut stop: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => return,
            _ = tick.tick() => {
                if *stop.borrow() { return; }
                if run_session(stop.clone()).await.is_err() {
                    // Never log database/parser errors: they may include statement
                    // contents, connection credentials, or provider response data.
                    log::error!("Statement queue session stopped; retrying recovery on the next tick");
                }
                if *stop.borrow() { return; }
            }
        }
    }
}

async fn run_session(stop: watch::Receiver<bool>) -> anyhow::Result<()> {
    let (db, connection) = db_connection().await?;
    tokio::pin!(connection);
    let (lost, receiver) = watch::channel(false);
    let session = run_locked_session(&db, stop, receiver);
    tokio::pin!(session);
    tokio::select! {
        biased;
        _ = &mut connection => {
            // Dropping a blocking task's handle would NOT stop its parser.
            // Signal cancellation and join it before this session can return.
            let _ = lost.send(true);
            let _ = session.await;
            anyhow::bail!("Statement queue lock connection was lost");
        }
        result = &mut session => result,
    }
    // Both the client and its directly-driven connection are dropped here;
    // there is no detached lock owner or outstanding parser task.
}

async fn run_locked_session(
    db: &Client,
    stop: watch::Receiver<bool>,
    lost: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if !statement_import::try_lock(db).await? {
        return Ok(());
    }
    statement_import::recover_interrupted_locked(db).await?;
    while !*stop.borrow() && !*lost.borrow() {
        let Some(file) = statement_import::claim_next_locked(db).await? else {
            break;
        };
        let id = file.id;
        let outcome = run_import(db, file, stop.clone(), lost.clone()).await?;
        statement_import::finish_locked(db, id, outcome.error()).await?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum ImportOutcome {
    Succeeded,
    Failed(String),
    Panicked,
    Interrupted,
}

impl ImportOutcome {
    fn error(&self) -> Option<&str> {
        match self {
            Self::Succeeded => None,
            Self::Failed(message) => Some(message),
            Self::Panicked => Some(PANIC_ERROR),
            Self::Interrupted => Some(statement_import::INTERRUPTED_ERROR),
        }
    }
}

fn parser_error(error: anyhow::Error) -> String {
    // Preserve actionable parser diagnostics, but not database/provider source
    // chains, which can contain credentials, query values, or response bodies.
    let message = if error.chain().any(|source| source.is::<tokio_postgres::Error>()) {
        "Database write failed while importing the statement; check server database availability".to_owned()
    } else if error.chain().any(|source| source.is::<reqwest::Error>()) {
        "A provider request failed while importing the statement; check server configuration and provider availability".to_owned()
    } else {
        error.to_string().chars().take(2000)
            .map(|character| if character.is_control() { ' ' } else { character })
            .collect()
    };
    format!("{}. {PARTIAL_IMPORT_WARNING}", message.trim_end_matches('.'))
}

struct ParserTask {
    cancel: watch::Sender<bool>,
    task: JoinHandle<ImportOutcome>,
}

impl Drop for ParserTask {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

fn spawn_parser(file: ClaimedStatementImport) -> ParserTask {
    let (cancel, mut cancelled) = watch::channel(false);
    let task = tokio::task::spawn_blocking(move || {
        // PDF extraction and CSV parsing are synchronous. Keep them off the
        // API's current-thread executor, but retain async database access here.
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return ImportOutcome::Failed(format!("Could not start statement importer. {PARTIAL_IMPORT_WARNING}"));
        };
        runtime.block_on(async {
            if *cancelled.borrow() { return ImportOutcome::Interrupted; }
            tokio::select! {
                biased;
                _ = cancelled.changed() => ImportOutcome::Interrupted,
                result = AssertUnwindSafe(parse_file_for_upload(&file.contents, Path::new(&file.filename))).catch_unwind() => {
                    match result {
                        Ok(Ok(())) => ImportOutcome::Succeeded,
                        Ok(Err(error)) => ImportOutcome::Failed(parser_error(error)),
                        Err(_) => ImportOutcome::Panicked,
                    }
                }
            }
        })
    });
    ParserTask { cancel, task }
}

async fn cancel_and_join(parser: &mut ParserTask) {
    let _ = parser.cancel.send(true);
    let _ = (&mut parser.task).await;
}

async fn run_import(
    db: &Client,
    file: ClaimedStatementImport,
    mut stop: watch::Receiver<bool>,
    mut lost: watch::Receiver<bool>,
) -> anyhow::Result<ImportOutcome> {
    let mut parser = spawn_parser(file);
    let mut heartbeat = interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() || *lost.borrow() {
            cancel_and_join(&mut parser).await;
            return Ok(ImportOutcome::Interrupted);
        }
        tokio::select! {
            biased;
            _ = lost.changed() => {
                cancel_and_join(&mut parser).await;
                return Ok(ImportOutcome::Interrupted);
            }
            _ = stop.changed() => {
                cancel_and_join(&mut parser).await;
                return Ok(ImportOutcome::Interrupted);
            }
            _ = heartbeat.tick() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(10), db.simple_query("SELECT 1")).await, Ok(Ok(_))) {
                    cancel_and_join(&mut parser).await;
                    anyhow::bail!("Statement queue lock heartbeat failed");
                }
            }
            result = &mut parser.task => {
                return Ok(result.unwrap_or(ImportOutcome::Panicked));
            }
        }
    }
}

