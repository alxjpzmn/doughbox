use super::db_helpers::db_client;
use tokio_postgres::Client;

async fn seed_trades_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists trades (
          hash TEXT NOT NULL UNIQUE,
          broker TEXT NOT NULL,
          date TIMESTAMP WITH TIME ZONE NOT NULL,
          no_units NUMERIC NOT NULL,
          avg_price_per_unit NUMERIC NOT NULL,
          eur_avg_price_per_unit NUMERIC NOT NULL,
          security_type TEXT NOT NULL,
          direction TEXT NOT NULL,
          currency_denomination TEXT NOT NULL,
          isin TEXT NOT NULL,
          date_added TIMESTAMP WITH TIME ZONE NOT NULL,
          fees NUMERIC NOT NULL DEFAULT 0.0,
          withholding_tax NUMERIC,
          witholding_tax_currency TEXT
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_fund_reports_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists fund_reports (
          id integer NOT NULL UNIQUE,
          date TIMESTAMP WITH TIME ZONE NOT NULL,
          isin TEXT NOT NULL,
          currency TEXT NOT NULL,
          dividend NUMERIC NOT NULL DEFAULT 0.0,
          dividend_aequivalent NUMERIC NOT NULL DEFAULT 0.0,
          withheld_dividend NUMERIC NOT NULL DEFAULT 0.0,
          intermittent_dividend NUMERIC NOT NULL DEFAULT 0.0,
          wac_adjustment NUMERIC NOT NULL DEFAULT 0.0
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_fx_rates_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists fx_rates (
            hash TEXT NOT NULL UNIQUE,
            date date NOT NULL,
            rate NUMERIC,
            currency_from TEXT NOT NULL,
            currency_to TEXT NOT NULL
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_stock_splits_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists stock_splits (
          id TEXT NOT NULL UNIQUE,
          ex_date TIMESTAMP WITH TIME ZONE NOT NULL,
          from_factor NUMERIC NOT NULL,
          to_factor NUMERIC NOT NULL,
          isin TEXT NOT NULL,
          date_added TIMESTAMP WITH TIME ZONE NOT NULL
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_fx_conversion_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists fx_conversions (
          id TEXT NOT NULL UNIQUE,
          date TIMESTAMP WITH TIME ZONE NOT NULL,
          broker TEXT NOT NULL,
          from_amount NUMERIC NOT NULL,
          to_amount NUMERIC NOT NULL,
          from_currency TEXT NOT NULL,
          to_currency TEXT NOT NULL,
          date_added TIMESTAMP WITH TIME ZONE NOT NULL,
          fees NUMERIC,
          withholding_tax NUMERIC,
          witholding_tax_currency TEXT
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_listing_changes_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists listing_changes (
          id TEXT NOT NULL UNIQUE,
          ex_date TIMESTAMP WITH TIME ZONE NOT NULL,
          from_factor NUMERIC NOT NULL,
          to_factor NUMERIC NOT NULL,
          from_identifier TEXT NOT NULL,
          to_identifier TEXT NOT NULL
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_instruments_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists instruments (
          id TEXT NOT NULL UNIQUE,
          last_price_update TIMESTAMP WITH TIME ZONE NOT NULL,
          price NUMERIC NOT NULL,
          name TEXT NOT NULL
            )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_performance_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists performance (date TIMESTAMP WITH TIME ZONE NOT NULL,total_value NUMERIC NOT NULL,total_invested NUMERIC NOT NULL)",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_dividends_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists dividends (
                id text PRIMARY KEY,
                date TIMESTAMP WITH TIME ZONE NOT NULL,
                isin text NOT NULL,
                amount NUMERIC NOT NULL,
                broker TEXT,
                currency TEXT,
                amount_eur NUMERIC NOT NULL,
                withholding_tax NUMERIC,
                witholding_tax_currency TEXT
                )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_interest_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists interest (
                id text PRIMARY KEY,
                date TIMESTAMP WITH TIME ZONE NOT NULL,
                amount NUMERIC NOT NULL,
                broker TEXT,
                principal TEXT,
                currency TEXT NOT NULL,
                amount_eur NUMERIC NOT NULL,
                withholding_tax NUMERIC,
                witholding_tax_currency TEXT
                )",
            &[],
        )
        .await?;

    Ok(client)
}

async fn seed_ticker_conversion_db() -> anyhow::Result<Client> {
    let client = db_client().await?;

    client
        .execute(
            "CREATE TABLE if not exists ticker_conversions (
                id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
                ticker text NOT NULL,
                isin text NOT NULL
                )",
            &[],
        )
        .await?;

    Ok(client)
}

pub async fn run_migrations() -> anyhow::Result<()> {
    seed_listing_changes_db().await?;
    seed_fx_rates_db().await?;
    seed_fx_conversion_db().await?;
    seed_ticker_conversion_db().await?;
    seed_fund_reports_db().await?;
    seed_stock_splits_db().await?;
    seed_trades_db().await?;
    seed_dividends_db().await?;
    seed_interest_db().await?;
    seed_performance_db().await?;
    seed_instruments_db().await?;
    Ok(())
}
