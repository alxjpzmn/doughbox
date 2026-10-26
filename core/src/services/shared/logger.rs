use std::sync::{LazyLock, Mutex};

use anyhow::Context;
use tracing_subscriber::{
    filter::LevelFilter, layer::SubscriberExt, reload, util::SubscriberInitExt, Registry,
};

use super::settings::parse_log_level;

struct LogController {
    handle: reload::Handle<LevelFilter, Registry>,
    current: Mutex<LevelFilter>,
}

static CONTROLLER: LazyLock<LogController> = LazyLock::new(|| {
    let (filter, handle) = reload::Layer::new(LevelFilter::INFO);
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
    LogController {
        handle,
        current: Mutex::new(LevelFilter::INFO),
    }
});

pub fn init_logger() {
    LazyLock::force(&CONTROLLER);
}

pub fn apply_level(value: &str) -> anyhow::Result<()> {
    let level = parse_log_level(value)?;
    let controller = &*CONTROLLER;
    let mut current = controller
        .current
        .lock()
        .map_err(|_| anyhow::anyhow!("Logger filter lock poisoned"))?;
    if *current != level {
        controller
            .handle
            .reload(level)
            .context("Reloading log filter")?;
        *current = level;
    }
    Ok(())
}
