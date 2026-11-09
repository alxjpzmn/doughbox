# Doughbox

## What is this?

Doughbox is a self-hosted portfolio tracker. It lets you import various brokerage statements (more on supported brokers below), stores them all in a unified format in a Postgres instance and then displays various stats on your portfolio, either via CLI or web interface (which can also be used as PWA on your phone).

If you need to take care of capital gains taxes in Austria, you can use Doughbox to help you prepare your tax declaration. Beware that I'm not a tax advisor and I take no responsibility for the outcome — you're responsible for your taxes, so: read the code, do your own research and consult a professional tax advisor.

<img alt="Screenshots of Doughbox" src="./.github/doughbox-screenshots.png" style="width: 100%" />

## Features

- Self-hosted, with a Docker image for easy deployment.
- Web interface, CLI and API.
- Support for FX conversions and trades in non-Euro currencies with FX rates from the ECB.
- Optional automatic EUR equity/ETF portfolio prices from Xetra, Tradegate, Frankfurt, Cboe Europe and Euronext, using a durable Postgres queue with scheduling and worker limits managed in the web UI. The same bulk imports maintain market-wide latest-trade references for MCP.
- Import brokerage statements from various brokers, e.g. Trade Republic, Revolut, Trading 212, etc.
- Connect your an agent via MCP to make orders against your Trading 212 Account (demo or live supported); you approve or discard them in **Trading** before any broker submission.
- Display Portfolio Allocation across brokerage accounts.
- Display returns over time for the portfolio and individual positions (both open and closed).
- Display a consolidated timeline across brokers of past trades, interest & dividend payments and fx conversions.
- Go back in time and view your past portfolio's holdings.
- With a FRED token, compare securities P&L including recorded dividends against a nominal S&P 500 price-index proxy using the same gross purchase budget and sold units; the benchmark has no simulated fees, taxes, currency returns or dividend reinvestment.
- Fetches stock split data from polygon.io (hence, you will need an API token from Polygon).
- Username/password accounts with mandatory authenticator-app two-factor authentication, private portfolios, administrator-issued invitations, and email-free recovery.
- Transactions receive an ID from the broker or a combination of transaction characteristics. Reimporting a statement deduplicates within the same account; different users can import identical statements independently.
- Austrian tax preparation with fee-exclusive moving-average EUR security costs per broker/ISIN, separate gross income and estimated withholding credits.
- Instance-operated Telegram bot with private per-user destinations and daily report schedules; manual reports use `--user <username> portfolio --notify`.

## (Known) Limitations

- EUR as base currency — it supports trades in other currencies, but the base currency needs to be EUR.
- Automatic portfolio valuation remains opt-in and EUR-only for instruments imported as `Equity` (including ETFs); known bonds and derivatives remain excluded. MCP last-trade references are delayed, not live/executable quotes, bid/ask books, an NBBO, or official closing prices. Missing reports, paused schedules and feed failures can leave references stale or unavailable.
- Doughbox uses ISINs as primary identifier, so it's agnostic to the trading venue an instrument was bought. If it encounters a ticker symbol during the import, it attempts to convert it to an ISIN via the OpenFIGI API
- Crypto isn't supported.
- Custom physical gold, real estate, private loans, and interest-bearing cash accounts can be tracked manually in the web UI. They aren't considered for Taxation.
- Precomputed performance and taxation need to be refreshed explicitly from the web client or the CLI.
- Tax reports are preparation aids, not final Austrian tax payable. Read their `limitations`: complete acquisition history, brokerage portfolio identity, cash FX, income classification, fund-report coverage, treaty eligibility and annual loss offsets need review.
- Trade execution assumes an EUR base account; commission currency is handled separately.

## Supported brokers

| Broker               | Equities & ETFs | Bonds         | Derivatives   | Interest      | FX conversions |
| -------------------- | --------------- | ------------- | ------------- | ------------- | -------------- |
| Revolut              | ✅              | ⚠️ Might work | n.a.          | ⚠️ Might work | ✅             |
| Trade Republic       | ✅              | ✅            | ✅            | ✅            | n.a.           |
| Scalable             | ✅              | n.a.          | ⚠️ Might work | ❌            | n.a.           |
| Erste Bank / George  | ✅              | ⚠️ Might work | ⚠️ Might work | n.a.          | ❌             |
| Wise                 | n.a.            | n.a.          | n.a.          | ✅            | ✅             |
| Interactive Brokers  | ✅              | ✅            | ✅            | n.a.          | ✅             |
| Lightyear            | ✅              | n.a.          | n.a.          | ✅            | ✅             |
| Trading 212 (Invest) | ✅              | n.a.          | n.a.          | ✅            | ✅             |

## Supported price feeds

| Exchange / feed | Coverage                                                                                                   | Feed key             |
| --------------- | ---------------------------------------------------------------------------------------------------------- | -------------------- |
| Xetra           | Deutsche Börse delayed post-trade reports                                                                  | `DETR-posttrade`     |
| Tradegate       | Deutsche Börse delayed post-trade reports                                                                  | `DGAT-posttrade`     |
| Frankfurt       | Deutsche Börse delayed post-trade reports                                                                  | `DFRA-posttrade`     |
| Cboe Europe     | BXE, CXE and DXE exchange streams; APA/TRF reporting excluded                                              | `CBOE-posttrade`     |
| Euronext        | Equities and ETFs/funds across Amsterdam, Brussels, Dublin, Lisbon, Milan, Oslo and Paris, where available | `EURONEXT-posttrade` |

Administrators select exchanges and manage refresh/reset schedules in the instance controls. Members can read shared feed status and their own quotes, but cannot control public workers. All five bulk feeds maintain one compact latest reference per `(ISIN, currency, feed)` across the market, including unheld instruments and empty portfolios. This does not create unrelated instruments, trades or accounting records. Portfolio source-report history is admitted from the equity/ETF ISIN union, while instrument names, pins, execution fallbacks and selected quote projections remain private to each owner.

Compact storage keeps only current `mifid_latest_price` heads, not market-wide original-report history. `mifid_price_revision` keeps amendment/cancellation identity guards, so late originals cannot resurrect revoked references. Invalidating a persisted head also saves its full ordering boundary in `mifid_price_floor`. Until a candidate reaches that boundary, MCP returns unavailable rather than selecting a lower runner-up or backdated amendment: discarded competitors from earlier files cannot be reconstructed, even by replaying the original head. Fresh-file amendments/cancellations are still resolved against all per-file candidates before selecting a head.

| Stored data                                   | Retention / pruning rule                                                                                                                          |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Latest reference per `(ISIN, currency, feed)` | Expires exactly **168 hours after its source event**; replay, receipt and `max_age_seconds` never extend it.                                      |
| Amendment/cancellation identity guards        | Seven days after source publication; replay does not extend retention.                                                                            |
| Recency floors for unavailable references     | Removed when a defensible head replaces them, or 168 hours after their execution-time boundary. V32 upgrade floors use at least the upgrade time. |
| Portfolio source reports/tombstones           | Seven days after receipt/latest revision, unless selected by a portfolio instrument. Selected references stay protected.                          |
| File checkpoints                              | Seven days after processing.                                                                                                                      |
| Completed/failed jobs                         | 14 days after completion; active jobs are not pruned.                                                                                             |

Startup/hourly cleanup drains batches of at most **5000 rows per table per transaction**, including all three compact tables, even when schedules are paused or feeds fail; successful jobs also run one cleanup batch. Reads enforce event/split validity before maintenance. Expiring a reference is not a delisting decision and never deletes instrument or accounting history. Keep PostgreSQL autovacuum enabled to reclaim/reuse space from updates and pruning; deleting rows does not guarantee that database files shrink on disk.

## Why does this exist?

Doughbox is scratching my own itch: I have lots of different brokerage accounts and I wanted an easy way to stay on top of my portfolio allocation and its performance. While there are great portfolio tracking apps out there (Delta, Finanzfluss Co-Pilot, Parqet, Quin), I wanted to have full control over the data and the supported brokers. And when the need for some custom visulations and capital gains tax preparation came about, I started developing Doughbox. Since I use a lot of open source, self-hosted services, it makes sense for me to open source my personal infrastructure, too.

## Get Started

Doughbox requires PostgreSQL **15 or newer** (CI uses 17). Accounts use an immutable canonical username, password and mandatory authenticator-app TOTP; no email service is required. Each account owns a private portfolio. Administrator status permits account and instance operations, not access to another user's financial data. A polygon.io API key is needed for stock-split housekeeping, but not for the API or price refreshes.

### Environment variables

| Variable                   | Required                 | Purpose                                                                                                         |
| -------------------------- | ------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `POSTGRES_URL`             | database commands/API    | PostgreSQL connection string; `--help` needs no database                                                        |
| `AUTH_MASTER_KEY`          | API and `users` commands | Persistent 32-byte auth encryption key, exactly 64 hexadecimal characters                                       |
| `DOUGHBOX_PUBLIC_URL`      | API and `users` commands | Canonical browser origin, e.g. `https://doughbox.example.com`; no credentials, path, query or fragment          |
| `AUTH_ALLOW_INSECURE_HTTP` | no                       | Set exactly `true` only for local/trusted-network HTTP; HTTPS is otherwise required                             |
| `DOUGHBOX_DATA_DIR`        | no                       | Persistent data root; defaults to `.` relative to the process working directory                                 |
| `POLYGON_TOKEN`            | for stock splits         | Stock-split data from polygon.io                                                                                |
| `FRED_TOKEN`               | no                       | S&P 500 benchmark via FRED                                                                                      |
| `TRADING_MASTER_KEY`       | for trading              | Separate persistent 32-byte broker encryption key, encoded as 64 hexadecimal characters                         |
| `TRADING_MCP_HOSTS`        | for remote MCP           | Comma-separated allowed MCP hosts; defaults to `localhost,127.0.0.1,::1`; an explicitly empty value is rejected |
| `TG_TOKEN`                 | no                       | Instance-operated Telegram bot credential; each user configures their own chat destination in Settings          |
| `TG_CHAT_ID`               | legacy upgrade only      | Imported once into the reserved legacy owner's destination; never a runtime fallback                            |
| `RUST_ENV`                 | no                       | Loads `.env.dev` for `development` (default), `.env.prod` for `production`                                      |

Generate each encryption key once with `openssl rand -hex 32` and keep the value in persistent secret configuration. Never generate keys during container startup. `PASSWORD`, `API_TOKEN` and `SESSION_TOKEN_KEY` are retired and never authorize requests.
Missing/malformed keys fail startup. A changed/lost auth key requires deliberate console recovery and authenticator re-enrollment; it never enables password-only login. When changing Docker mounts, keep the existing output directory at `<DOUGHBOX_DATA_DIR>/output` so recognized legacy artifacts are migrated.

### Accounts, setup and offline recovery

1. Start the configured API. An uninitialized installation displays a setup-needed login screen; the first visitor cannot claim it.
2. On the trusted server console, run `./doughbox users bootstrap` (Docker: `docker exec -it doughbox ./doughbox users bootstrap`). It prints a single-use, 24-hour setup URL once. Share/open it manually; normal API startup prints no setup credential.
3. Choose a username and password, then enroll an authenticator using the locally generated QR or manual secret. Save the ten one-time recovery codes before continuing. Password success alone never grants portfolio access.
4. Administrators issue seven-day member invitations in **Users** and share the one-time links manually. There is no open signup or portfolio impersonation. New members start empty, with private schedules paused and broker/notification settings unconfigured.

Usernames are lowercased ASCII, 3–32 characters, starting with a letter/digit and otherwise allowing letters, digits, `_`, `.` and `-`. They cannot be renamed. Passwords contain 12–128 Unicode scalar values and at most 512 UTF-8 bytes; whitespace, case and Unicode are preserved.

**Security** manages passwords, authenticator replacement, recovery codes, sessions and personal tokens. Sensitive account/admin operations require password plus a second factor verified within five minutes. TOTP uses six digits and 30-second periods with a ±1-step window and replay protection: a code accepted for enrollment/login/reauthentication cannot be reused. Keep server/device clocks synchronized. Recovery codes replace the OTP factor, never the password. There is no active-2FA-off control.

Same-user reauthentication preserves financial caches and form drafts. Authentication errors from requests dispatched under superseded credentials cannot invalidate the newly verified session or dismiss newly issued recovery codes.

A web-admin password-reset link lasts one hour and still requires the account's existing authenticator or an unused recovery code. If both password/factors are lost, a trusted operator runs `./doughbox users recover --username alice`; add `--enable` explicitly for a disabled account. This revokes old credentials and prints a single-use, 30-minute recovery link requiring a new password and fresh TOTP enrollment. Financial history and notification destinations are retained. An interrupted enrollment resumes with the chosen username/password.

Registration and recovery forms stay bound to the link inspected in that tab. Reload resumes only the same binding; only its non-secret ID is kept in the history entry. If another tab changes the shared session's link, the original form clears its sensitive draft and requires reopening the original full link instead of adopting the other account or invitation.

### Upgrading an existing installation

Back up PostgreSQL, `DOUGHBOX_DATA_DIR`, `AUTH_MASTER_KEY` and, if used, `TRADING_MASTER_KEY` separately. Stop all old API/CLI workers before starting the cutover binary; never mix old unscoped writers with the new schema. V33 preserves V1–V32 and assigns existing private rows to a reserved pending administrator, which only the console bootstrap link can activate.

Before serving or starting workers, recognized legacy reports in `<data-dir>/output` move to `<data-dir>/output/users/<server-generated-user-id>/`. Only `performance.csv`, `performance.json`, `taxation.json` and valid generated detailed-tax report names are migrated; unknown files and CLI input directories are untouched. Interrupted migration resumes safely; differing source/destination files stop startup without overwriting either. Existing `TG_CHAT_ID` is imported once for this reserved owner. Bootstrap/enrollment do not reset migrated schedules, broker ciphertext or history.

All new saved reports use that per-user directory. Missing own saved reports return `404`, never another account's or the old root artifact. Multiple report-serving replicas require the same mounted `DOUGHBOX_DATA_DIR`; PostgreSQL shares sessions and identities, not local files. Without a shared volume, use one report-serving process. Server/database operators remain trusted; account roles do not cryptographically prevent operator access.

### Agent-assisted trading

Trading has two sections: **Orders** for pending approvals and recent order history, and **Setup** for the Trading 212 connection and agent access. Orders is listed first and opens by default when setup is complete or proposals/history already exist; a new account starts in Setup. Approval links open Orders directly; signed-out users on mobile and desktop are sent through login with the proposal return target preserved. Switching tabs preserves unsaved setup inputs and does not submit orders or change the connection.

1. Sign in to your fully enrolled account and set a persistent `TRADING_MASTER_KEY` on the server, then restart. Generate the broker encryption key once with `openssl rand -hex 32`; keep it separately from `AUTH_MASTER_KEY` and back it up independently of PostgreSQL. Losing or changing it makes saved broker credentials unreadable.
2. Open **Trading → Setup**, select **Demo** initially, and enter the Trading 212 **API key ID** and **Secret Key**. Only Trading 212 is supported for now. Create the key pair with the [API key setup](https://helpcentre.trading212.com/hc/en-us/articles/14584770928157-Trading-212-API-key) and see the [API docs](https://docs.trading212.com/api). Trading 212 uses HTTP Basic authentication with both fields. Grant account/metadata read and order-placement permissions. Doughbox verifies account access and saves a stock/ETF instrument catalog before connecting; missing order permission will cause approval submissions to fail.
3. In **Setup**, create an agent token after recent password-plus-factor reauthentication and copy it immediately. Only its hash and captured account version are stored; plaintext is displayed once in the tab, never browser storage. Rotation immediately revokes the old token.
4. Add the displayed **Streamable HTTP MCP endpoint**, `/api/trading/mcp`, to the external agent with `Authorization: Bearer <agent token>` on every request. Never give an agent broker credentials, your password, authenticator secret or recovery codes. Use the trading token for MCP; use a separate **read-only personal API token** for REST clients (see [REST API](#rest-api)). Agent credentials authorize reading your whole recorded portfolio, trades and activity across brokers (including manual assets and cash), instrument search, quotes and trade proposals, not owner approval, account administration or portfolio REST routes.
5. Review incoming proposals in **Trading → Orders**. **Approve order / Approve entire batch** submits the reviewed limit orders immediately; **Discard** sends nothing to the broker. Live mode is explicitly labelled. `submit_trades` and `get_proposal` return an owned proposal's `approval_url` using configured `DOUGHBOX_PUBLIC_URL`, never request Host/forwarded headers. Opening it only focuses the proposal; it does not approve or submit. With `TG_TOKEN` and your own chat destination configured, a newly saved proposal sends that link to your Telegram chat; no daily schedule is required for this alert. Connections, catalogs, replays, history and execution locks are per user.

A typical agent-client configuration is:

```json
{
  "mcpServers": {
    "doughbox-trading": {
      "url": "https://doughbox.example.com/api/trading/mcp",
      "headers": { "Authorization": "Bearer <agent token>" }
    }
  }
}
```

The MCP surface exposes:

| Tool                 | Purpose                                                                                                                                                                                                                                                                        |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `get_portfolio`      | Read the owner's current recorded portfolio across all brokers, including securities, cash and manual assets, with EUR values, quantities and allocations. No arguments; does not fetch live broker balances or refresh prices.                                                |
| `get_trades`         | Read recorded imported security trades, filtered by inclusive UTC dates, canonical `isin`, exact `broker` and `direction` (`Buy`/`Sell`), with bounded pagination.                                                                                                             |
| `get_activity`       | Read the recorded unified timeline, including manual holdings, income, FX, balances and valuations, filtered by inclusive UTC dates, exact `identifier`, `broker` and `event_type`, with bounded pagination.                                                                   |
| `search_instruments` | Search the connected stock/ETF catalog by ticker, name, or ISIN using `query`; returns exact Trading 212 tickers, ISINs, and native currencies.                                                                                                                                |
| `get_quotes`         | Read delayed bulk-ingested last-trade prices for 1–50 exact Trading 212 `tickers`, with native provenance, source-event age and stored dated FX conversion.                                                                                                                    |
| `submit_trades`      | Save one or up to 50 limit trades as a proposal, never execute them. The result includes `approval_url` for the signed-in Trading page. Opening it does not submit the order. A new proposal also requests a Telegram notification with that link when Telegram is configured. |
| `get_proposal`       | Read a proposal's review/submission status by `id`. Pending results include the same `approval_url`; opening it does not approve the order.                                                                                                                                    |

For holdings, call `get_portfolio` with `{}` arguments using the existing trading-agent token.

The result reuses the dashboard's `PortfolioOverview`, plus `currency: "EUR"` and a `notice`: `generated_at` is calculation time (Unix seconds), **not** a price timestamp or proof of broker synchronization. `positions` includes each holding's `asset_id`, nullable `isin`, `asset_class`, `name`, exact decimal-string `units`, `unit_label`, nullable EUR `value` and percentage `share`. Cash units are native-currency balances. Quantities reflect imported/manually recorded activity; values use stored prices and FX, not live market quotes or a saved weekly report. Missing valuations remain `null`; totals and allocations include known values only. Check `valuation_complete`, `unpriced_isins`, `unpriced_assets`, `returns_incomplete` and `security_history_gaps` before treating totals or returns as complete.

For recent transactions, call `get_trades` or `get_activity` with `{}` to read the last 30 calendar days through today. Both accept inclusive `from_date` / `until_date` in `YYYY-MM-DD`; if only `until_date` is supplied, the default start is 29 days earlier. The entire final UTC day is included, including fractional seconds. Pages default to `limit: 50` (allowed 1–100) and `offset: 0`. For example, `get_trades`:

```json
{
  "from_date": "2026-11-01",
  "until_date": "2026-11-09",
  "direction": "Buy",
  "limit": 25
}
```

- Both return `items`, `generated_at` (Unix seconds), resolved `from_date` / `until_date`, `limit`, `offset`, nullable `next_offset`, and a `notice`. Follow `next_offset` with the **same filters and returned date bounds** until it is `null`. Results are newest first with deterministic tie-breakers. Offset pages are not an immutable snapshot: edits, deletions or backdated imports between requests can shift rows.
- `get_trades` preserves recorded transaction quantities/prices and canonicalizes renamed ISINs. Prices use each trade's `currency`; `eur_avg_price_per_unit` and `fees` are EUR. This tool covers imported security trades, not manual asset trades, proposals or broker order status.
- `get_activity` shares the dashboard timeline's event calculations, including split-adjusted security trade units/prices. Use `get_trades` when the original transaction quantity is required. `identifier` matches the displayed event identifier: canonical ISIN, manual asset ID, FX pair such as `EURUSD`, or numeric OeKB report ID as a string. Brokers match exactly; manual balances, transactions and valuations use `Manual`.
- Activity `event_type` accepts `Trade`, `Dividend`, `CashInterest`, `ShareInterest`, `PrivateDebtInterest`, `FxConversion`, `DividendAequivalent`, `Deposit`, `Withdrawal`, `PrincipalAdvance`, `PrincipalRepayment`, `OpeningBalance`, `BalanceReconciliation`, or `Valuation`. Check each row's `currency`, `total_currency`, `direction` and `unit_label`. Balances, valuations and fund tax reports are **not** cash flows or income; never sum unlike event types into returns.
- Both tools read only recorded PostgreSQL data. They never fetch broker activity, import statements, refresh prices, fetch FX, execute orders or grant approval rights. Activity conversions use stored FX only; missing required observations return an error rather than fetching or guessing. Shared fund reports are limited to instruments recorded in the owner's portfolio.

Before proposing limit orders, call `get_quotes`:

```json
{ "tickers": ["AAPL_US_EQ", "VODl_EQ"], "max_age_seconds": 1800 }
```

- Mapping comes from Trading 212's [instrument metadata endpoint](https://docs.trading212.com/api/instruments/instruments). Older saved catalogs are backfilled on first use without reconnecting, discarding proposals, or rotating the token. Metadata backfill attempts are spaced at least 60 seconds apart.
- The batch returns `delayed: true`, `reference_type: "last_trade"`, `max_age_seconds`, `quotes` in request order **including duplicates**, and `notice`. Each quote has a nullable decimal-string **`price`** in the ticker's exact order `currency`, including **GBX pence**. There are no `bid`, `ask`, `midpoint`, `last_known` or `refresh_error` aliases. Refresh/reconnect MCP clients after upgrading to discover this changed contract.
- One native `reference` retains decimal-string `price`, nullable `quantity`, currency, the exact bulk `feed` key from the table above, source `"Bulk post-trade files"`, venue, conservative source-event `as_of`, and `received_at`. Receipt is provenance, not freshness. No bid/ask, retail flag, side timestamps or publication timestamp is invented from a last trade.
- Selection prefers non-indicative references, then those within `max_age_seconds` (positive; default 1800), then requested currency/exact GBP–GBX units, then EUR over other convertible currencies, then latest conservative source-event time. Equal candidates use lexical feed, currency and venue order for deterministic results; prices are never blended across reports.
- `age_seconds` and `stale` describe source-event age; exactly `max_age_seconds` is not stale. `indicative: true` means the selected report explicitly has quantity zero; unknown quantity remains null and is not assumed zero. An indicative reference can still have a price, and this flag is independent of staleness. `unavailable_reason` explains absent references or failed conversion. No reference survives 168 hours, a future execution/publication time or an effective stock split merely because it was recently received or a large age was requested.
- Native prices need no FX. Conversion uses stored effective rates on the reference event date and retains actual ECB observation dates. GBP/GBX uses exact 100-pence-per-pound factors; other pairs may cross via EUR. Missing/overflowing FX leaves the order-currency `price` null and preserves the native reference. Quotes never import FX; the existing ECB job warms catalog, portfolio and market-wide latest-reference currencies under **Data → Reference Data**.
- Quote pricing reads PostgreSQL only: **no exchange quote HTTP, bulk downloads, price-job enqueue or FX fetching**. Existing broker catalog metadata backfill remains possible for older connections. All five supported bulk feeds contribute references for unheld instruments as well as holdings, but only completed background/manual imports populate them. Paused/disabled schedules, unselected feeds, outages, absent reports or expired references mean stale/unavailable results; a quote request never repairs coverage itself.
- The agent must choose explicit ticker-currency `limit_price` values for **human review**. A last trade is only a delayed reference: quote lookup never chooses, copies, submits or reprices an order, or fetches broker-position marks. It is not a promise that Trading 212 can execute at that price.

Example `submit_trades` arguments (one item for an individual trade; multiple items for a batch):

```json
{
  "client_request_id": "agent-review-2026-001",
  "title": "Proposed Apple purchase",
  "note": "Explain the trade for the human reviewer.",
  "trades": [
    {
      "ticker": "AAPL_US_EQ",
      "side": "buy",
      "quantity": "0.5",
      "limit_price": "150.00",
      "time_validity": "DAY"
    }
  ]
}
```

Use tickers returned by `search_instruments`, not generic exchange symbols. Quantities and limit prices are positive, exact decimal strings; `side` is `buy` or `sell`. Prices use the instrument's native currency, including **GBX (pence), not GBP**, where applicable. `time_validity` is `DAY` (default) or `GOOD_TILL_CANCEL`. Market orders and automatic execution are intentionally absent.

Fractional limit orders are supported, but accepting exact decimal strings does **not** mean Trading 212 accepts arbitrary quantity precision. Its [fractional-share rules](https://helpcentre.trading212.com/hc/en-us/articles/9511997937437-Can-I-trade-with-fractional-shares) depend on instrument price and the existing position: published examples allow two quantity decimals at prices 1 to below 10, three at 10 to below 100, and four at 100 to below 1,000; full-position sales may have exceptions. Do not assume four decimals for every instrument. A delayed quote or user-selected limit is not authoritative broker metadata for this rule. Choose broker-valid quantities explicitly **before human approval**; Doughbox never rounds approved quantities/limits or changes an order to make a rejected request succeed.

After partial batch success, submitted/open orders remain active at Trading 212; they are not rolled back when a later trade fails. Do not include those accepted orders in a replacement batch, which could duplicate them. Any remaining trades require their own explicit review.

If a mutation succeeds but the following status refresh fails, the trading tab pauses further mutations instead of restoring an actionable stale state. Use **Retry / refresh status** to reconcile without repeating the mutation. A newly created token remains in tab memory while paused and is shown again after a successful refresh; rotating it resets the token's copy acknowledgement. Definite connection-validation rejections leave the form editable.

Replacing/disconnecting the broker discards pending proposals and revokes agent access, but never cancels existing broker orders. Token rotation alone preserves proposals. Recent history tracks broker-reported order status; manage cancellations in Trading 212 and continue importing statements for portfolio/tax accounting. The tab shows all pending proposals plus the latest 50 processed/discarded proposals; older records remain in PostgreSQL.

`Open` is checked against Trading 212's [pending orders API](https://docs.trading212.com/api/orders/orders) (`GET /api/v0/equity/orders`); it is not inferred from successful submission. Orders no longer present there are checked against the latest 50 historical orders. The page refreshes every 10 seconds while idle, with broker status requests spaced at least 6 seconds apart. Order-status reconciliation skips submitting batches and updates only matched orders after the batch finishes; approved amounts and broker order IDs are preserved. Failed polls preserve the last-known state, and absence from both responses is not treated as a fill or cancellation. Grant the broker key order-read and order-history permissions as well as placement access.

### Docker

```bash
  docker run -d --name doughbox \
  -e RUST_ENV=production \
  -e POSTGRES_URL="your_postgres_url_here" \
  -e AUTH_MASTER_KEY="your_persistent_64_hex_auth_key" \
  -e DOUGHBOX_PUBLIC_URL="https://doughbox.example.com" \
  -e DOUGHBOX_DATA_DIR="/data" \
  -p 8084:8084 \
  -v /path/to/your/input-directory:/app/input \
  -v /path/to/your/persistent-data:/data \
  ghcr.io/alxjpzmn/doughbox:latest
```

Serve the configured public origin through HTTPS/reverse proxy, then bootstrap the administrator as described above. Interactive API docs are at `/api/docs` (spec at `/api/openapi.json`). Browser traffic is same-origin; wildcard CORS is not enabled. Explicit insecure HTTP is only for local/trusted networks and cannot protect credentials on an untrusted plaintext connection.

Import statements through **Portfolio → Timeline → Import**, or run `./doughbox --user alice import foldername` from the trusted console after Alice is active and fully enrolled. CLI ownership is always explicit, even on single-account installations.

Inside Docker, the input path is the mounted container path: `docker exec -i doughbox ./doughbox --user alice import ./input`. Initial non-EUR imports may fetch historical FX rates.

After the first import, you will be prompted to run housekeeping and some portfolio and PL calculations, unless you called the import command with `--silent`. Especially on the first run it's highly recommended to run at least `housekeeping` (which will e.g. fetch data on past stock splits based on your holdings).

The web interface will be available per default on port `8084`.

You can run the following commands in the CLI:

Financial commands `import`, `portfolio`, `performance`, `taxation`, `housekeeping`, and `prices set|unpin|status` all require `--user <canonical-username>`. The CLI resolves one active, fully enrolled owner and passes it through follow-on work; it does not perform remote password/TOTP login or infer the first account. `api`, `users`, `debug-pdf`, `prices refresh` and `prices worker` are instance/operator commands and reject irrelevant `--user`.

`housekeeping`: for fetching stock split data, tax data for funds and ETFs from the Austrian Control Bank (OeKB) and updated FX rates from the ECB.

`portfolio`: to show your current holdings and their allocation in the terminal. Pass `-n` / `--notify` to also send the summary via Telegram.

`performance`: calculates and exports returns for present and past securities, with an S&P 500 price-index comparison when a FRED token is configured. Saved results are displayed in the web interface. The web **Refresh performance** button also saves a fresh whole-portfolio history snapshot.

`taxation`: calculates realized gains and losses, dividends, interest, OeKB dividend equivalents and recorded withholding using perpetual WAC. It saves a reusable report/transaction bundle for the web interface. Optional flags: `--from YYYY-MM-DD`, `--until YYYY-MM-DD`, `--with-events` (per-transaction breakdown).

`debug-pdf <path>`: print extracted text from a PDF statement (useful when adding or debugging an importer).

`api`: the command to run the web server that serves both the web frontend and the API. The docker container automatically runs this.

`prices refresh [--feed xetra|tradegate|frankfurt|cboe|euronext] [--dry-run]`: enqueue the persisted feed selection (or one explicitly named feed) and wait for completion, or parse and validate available last-48-hour bulk files without writing reports/prices/checkpoints/jobs. Parsing includes an empty portfolio. Dry-run counts are **portfolio source-report admissions** (holdings/existing identities), not estimates of market-wide retained heads. In-memory admission state spans batches and files within each feed refresh, so later off-scope amendments/cancellations of an earlier admission remain countable without database writes. Persistent ingestion additionally reduces all reports into compact global latest references. Shows queue status and portfolio-report processing progress; does not prompt for FX updates. With no selected feeds, choose a feed explicitly or update the UI selection.

`--user alice prices status`: JSON summary of shared feed status plus only Alice's current holdings' price sources/timestamps.

`prices worker`: run queue consumers for all supported feeds without the HTTP server or scheduler. Useful for a separate worker container; automatic scheduling requires an `api` process and the persisted scheduler enabled. Each process applies the worker limit saved in the web UI.

`--user alice prices set <ISIN> --price 123.45 [--as-of 2026-10-15T12:00:00Z]`: pin Alice's positive manual **EUR per-unit** price. `--user alice prices unpin <ISIN>` removes only her override and reselects her eligible market quote or execution fallback.

### Telegram portfolio reports

Set the instance bot's `TG_TOKEN`, then connect your own chat ID in **Settings → Telegram notifications**. Disconnecting sets the destination to null; another user's destination and legacy `TG_CHAT_ID` are never implicit fallbacks. Missing bot/destination means unconfigured and no automatic delivery. Enable **Daily portfolio reports**, choose the UTC send time, and save. Each user's schedule starts paused at **18:00 UTC**, survives restarts and runs without a browser; pending/disabled users start no private scheduled work. Already claimed imports/calculations may finish under their original owner, and account disable does not cancel approved broker executions.

### Brokerage statement imports

Open **Portfolio → Timeline** and tap **Import** to import brokerage transactions for supported brokers. Drag one or more PDF/CSV files into the drop area, or use **Choose files**, then select **Upload and queue**. The drop area highlights while files are dragged over it. **Hide imports** collapses the section without clearing selected files or stopping queue updates. Brokers are detected automatically. Uploads accept up to 100 non-empty files per batch, 25 MiB per file, and 99 MiB combined; the API allows a 100 MiB multipart body including headers. Invalid uploads reject the entire batch before enqueueing any file. Keep the page open until upload confirmation; afterwards the API processes the files without the browser.

API clients can use `POST /api/statement-imports` with repeated multipart fields named `files`, then poll `GET /api/statement-imports`. Both return `{"jobs":[...]}`; POST returns `202` after the whole batch is persisted. Job IDs are strings; each job includes its filename, byte size, status, queued/started/finished timestamps, and error when failed.

```bash
curl -H "Authorization: Bearer $DOUGHBOX_PERSONAL_WRITE_TOKEN" \
  -F 'files=@statement.csv' -F 'files=@statement.pdf' \
  http://localhost:8084/api/statement-imports
```

### REST API

Financial REST routes always select the authenticated owner's portfolio; request bodies/query parameters cannot choose an owner, and administrators have no financial bypass. **Security** issues personal read/write tokens lasting 90 days. Read permits own financial GETs; write additionally permits own private portfolio/import/calculation/notification mutations. Neither authorizes `/auth/*`, `/admin/*`, global worker controls or broker approval. Password/factor changes, console recovery, disabling and role changes revoke old browser/automation authority.

To connect a REST client or holdings-reading agent:

1. Open **Security → Personal API tokens**, also linked from **Trading → Setup → Manage REST API tokens**. A broker connection is not required.
2. Give the token a descriptive name and keep **Read** selected. Verify your password and second factor when requested, then explicitly retry **Create personal token**; creation is never automatically replayed.
3. Copy the new `doughbox_api_…` secret into your client's secret configuration. It is displayed once, never recoverable from the token list, and clears when you leave the page. Only its hash is stored. Do not put it in a URL, source code or shell history.
4. Send `Authorization: Bearer <personal API token>` on each REST request without a browser session cookie. Read permits `GET /api/portfolio` (securities, cash and manual assets with stored EUR valuations) and `GET /api/positions` (security quantities). These are recorded holdings, not broker synchronization or live market prices.
5. Revoke unused tokens in the same list. To replace one, create a new token, update the client, then revoke the old token. Tokens expire after 90 days; neither read nor write personal tokens authorize trading MCP or order approval.

For example, with `DOUGHBOX_API_TOKEN` supplied privately by your client/secret manager:

```bash
curl -H "Authorization: Bearer $DOUGHBOX_API_TOKEN" \
  "https://doughbox.example.com/api/portfolio"
```

Browser authentication uses `GET /api/auth/state`, username/password `POST /api/auth/login`, then mandatory `/api/auth/mfa/verify` or enrollment. Sessions are durable PostgreSQL records shared across API replicas, with 30-minute authenticated inactivity and a 12-hour absolute deadline. Cookies are HttpOnly/SameSite=Lax, Secure for the configured HTTPS origin. Every API response is `Cache-Control: no-store`.

Initial auth-state reads are coalesced within a browser tab. A supplied expired, revoked or rotated cookie returns `409 AuthenticationStateChanged` without a replacement cookie, so a delayed read cannot overwrite a newer session. Recheck the session with **Try again**; if the cookie is genuinely invalid, explicitly **Sign out** to clear it before starting a fresh anonymous session.

Every unsafe request using a valid cookie session, including empty-body and multipart requests, must send matching `Origin` and `X-CSRF-Token` from auth state. Keep CSRF in memory. Explicit `POST /api/auth/logout` may clear an absent/invalid session without CSRF, but still requires the configured Origin and rejects cross-site requests; valid-session logout retains normal CSRF checks. Token-only requests are CSRF-exempt; combining a bearer token with the session cookie is rejected. A stale session/CSRF transition must be revalidated, never automatically replayed—especially broker approval. Login/factor/link budgets are persisted across processes; failures return structured errors and `Retry-After` when throttled/unavailable. The retired `/api/login`, `/api/logout` and `/api/auth_state` routes are removed.

| Method               | Path                                                          | Purpose                                                                                                                                                           |
| -------------------- | ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`                | `/api/docs`                                                   | Interactive Swagger UI (no auth)                                                                                                                                  |
| `GET`                | `/api/openapi.json`                                           | OpenAPI spec                                                                                                                                                      |
| `GET`                | `/api/portfolio`                                              | Allocations and total return                                                                                                                                      |
| `GET`                | `/api/positions?date=&isin=&broker=`                          | Holdings as of a date                                                                                                                                             |
| `GET`                | `/api/trades?isin=&broker=&direction=&from_date=&until_date=` | Trades                                                                                                                                                            |
| `GET`                | `/api/buy-ins?isin=&broker=`                                  | Economic WAC per instrument/broker; `422 IncompleteSecurityHistory` when recorded acquisitions cannot cover a sale                                                |
| `GET`                | `/api/performance?isin=&broker=`                              | Live performance without file writes; `422 IncompleteSecurityHistory` rather than a fabricated cost basis                                                         |
| `GET`                | `/api/performance_overview`                                   | Own saved `performance.json` beneath the private output directory; `404` before calculation                                                                       |
| `GET`                | `/api/past_performance`                                       | Historical value snapshots                                                                                                                                        |
| `GET`                | `/api/performance/status`                                     | Persisted manual calculation status, timestamps and last error                                                                                                    |
| `POST`               | `/api/performance/refresh`                                    | Rebuild performance exports and save a history snapshot in the background; returns `202`, coalescing an already-running request                                   |
| `GET`                | `/api/timeline?start_date=&isin=&broker=&event_type=`         | Unified event timeline                                                                                                                                            |
| `GET`                | `/api/timeline/investment-activity?start_date=`               | Monthly net purchases, net cash added, and dividends & interest (EUR), optionally filtered from an inclusive UTC start date                                       |
| `GET`                | `/api/statement-imports`                                      | Your active and newest 100 completed uploads with per-file timestamps/errors; newest ID first                                                                     |
| `POST`               | `/api/statement-imports`                                      | Atomically queue repeated multipart `files` fields (PDF/CSV); returns `202` with accepted jobs; 100 files, 25 MiB each, 100 MiB total request                     |
| `GET`                | `/api/instruments` / `/api/instruments/{isin}`                | All known instruments with safe `quote` valuation/provenance metadata; existing top-level fields remain available                                                 |
| `PATCH`              | `/api/instruments/{isin}`                                     | Rename an existing instrument with `{"name":"Display name"}`; trims/rejects blank names without changing prices                                                   |
| `PUT`                | `/api/instruments/{isin}/price`                               | Pin a positive manual EUR unit price, e.g. `{"price":"123.456789","as_of":"2026-10-18T12:00:00Z"}`; `as_of` is optional and cannot be in the future               |
| `DELETE`             | `/api/instruments/{isin}/price`                               | Remove the manual pin and reselect stored market data or fallback                                                                                                 |
| `GET`                | `/api/prices`                                                 | Price schedule, queue errors and current holdings' quote provenance (no upstream requests)                                                                        |
| `PUT`                | `/api/prices/scheduler`                                       | Persist `{"enabled":true,"interval_minutes":30,"worker_concurrency":2,"feeds":["DETR-posttrade","DFRA-posttrade"]}`; apply without restart                        |
| `POST`               | `/api/prices/refresh`                                         | Queue the selected feeds, even while automatic scheduling is paused; returns `202` with current status                                                            |
| `POST`               | `/api/prices/feeds/{feed}/refresh`                            | Queue one supported feed; preserves active work and retry backoff                                                                                                 |
| `POST`               | `/api/prices/feeds/{feed}/reset`                              | Retire a queued/running job and queue a fresh attempt; preserve completed checkpoints, reports, prices, pins and history                                          |
| `GET`                | `/api/data-imports`                                           | Polygon/ECB availability, persisted per-source schedules, next runs, import status, timestamps, errors, stored counts and latest dates                            |
| `POST`               | `/api/data-imports/{source}/refresh`                          | Start `polygon` or `ecb` in the background; returns `202` with status, or `503` when the Polygon token is missing                                                 |
| `PUT`                | `/api/data-imports/{source}/scheduler`                        | Persist `{"enabled":true,"frequency":"monthly"}` for `polygon` or `ecb`; frequencies: `daily`, `weekly`, `monthly`; returns all source statuses                   |
| `GET`                | `/api/settings`                                               | Own `{telegram_token_set,telegram_chat_id,telegram_schedule}`; no instance log controls                                                                           |
| `PUT`                | `/api/settings/telegram`                                      | Set own destination with `{"chat_id":"..."}` or disconnect with `{"chat_id":null}`                                                                                |
| `GET` / `PUT`        | `/api/admin/settings`                                         | Administrator browser-only instance `{log_level}`; changes require recent reauthentication                                                                        |
| `PUT`                | `/api/settings/telegram-scheduler`                            | Save daily Telegram reports with `{"enabled":true,"send_time_utc":"18:00"}` (strict 24-hour UTC); returns settings including persisted `telegram_schedule` status |
| `GET`                | `/api/brokers`                                                | Distinct broker names (use these as filter values)                                                                                                                |
| `GET`                | `/api/dividends` / `/api/interest` / `/api/fx-conversions`    | Cash events                                                                                                                                                       |
| `GET`                | `/api/taxation?from_date=&until_date=`                        | Saved summary for the exact inclusive UTC date range; `404` if not calculated                                                                                     |
| `GET`                | `/api/taxation/transactions?from_date=&until_date=`           | Saved transaction impacts from the same calculation; never replays events                                                                                         |
| `GET`                | `/api/taxation/detailed?from_date=&until_date=`               | Saved report and transaction bundle for download; never recalculates                                                                                              |
| `GET`                | `/api/taxation/status`                                        | Persisted manual calculation status, timestamps, last error and actual requested range                                                                            |
| `POST`               | `/api/taxation/refresh?from_date=&until_date=`                | Calculate and save the selected range in the background; returns `202`, coalescing an already-running request                                                     |
| `GET`/`POST`         | `/api/assets`                                                 | List or create custom assets (`?include_archived=true` to include archived)                                                                                       |
| `GET`/`PUT`/`DELETE` | `/api/assets/{asset_id}`                                      | Custom asset detail, update, and archive                                                                                                                          |
| `POST`               | `/api/assets/{asset_id}/restore`                              | Restore an archived custom asset                                                                                                                                  |
| `GET`                | `/api/assets/holdings`                                        | Custom holdings and EUR values                                                                                                                                    |
| `GET`                | `/api/cash-accounts/summary`                                  | Volume-weighted cash interest                                                                                                                                     |

Portfolio responses combine securities and custom holdings. Missing security quotes have `value: null` and `share: null`, set `valuation_complete: false`, and are listed in `unpriced_isins`; aggregate returns must not be used in that state. Custom holdings with missing valuation or cost data set `returns_incomplete: true` and increment `unpriced_assets`; only known returns are included. The portfolio summary shows these conditions separately. CLI reporting and web performance refresh refuse to save a history snapshot when either condition is present.

Incomplete security acquisition history is reported separately in `security_history_gaps`: broker, canonical ISIN, first uncovered sale date, recorded inventory and sold quantity (split-adjusted shares). The portfolio remains available with `returns_incomplete: true` and **Known P&L**. Trading P&L for each affected canonical ISIN is excluded across all brokers; recorded dividends and gross purchases remain included. Holdings themselves may be incomplete, so reconcile quantities against broker statements and do not use the aggregate percentage while returns are incomplete.

Live performance and buy-ins return `422 IncompleteSecurityHistory` until the history is reconciled. Performance refresh reports the specific gap and refuses to overwrite exports or save a snapshot; taxation also remains strict about missing acquisition history. Active-only widgets are hidden because the active holding set cannot be determined reliably; saved full-portfolio results remain available. Verify the recorded sale quantity and import or reconcile the original acquisitions or transferred cost basis—never substitute a zero-cost purchase or ignore an oversale.

### Manual web calculations

- **Portfolio → Performance → Refresh performance** rebuilds the CSV/JSON securities exports and saves an income-inclusive, filterable portfolio history point. The saved widgets refresh when the job completes; leaving the page does not cancel the job.
- **Taxation → Recalculate taxation** saves the summary, transaction impacts and detailed export from one historical replay for the selected inclusive date range. Opening the page, switching tabs, changing dates and pressing **Export Detailed** only read saved results. An uncached range prompts an explicit calculation instead of calculating automatically.
- Full-history taxation is saved beneath `<data-dir>/output/users/<user-id>/` as `taxation_detailed.json` plus `taxation.json`. Selected ranges use `taxation_detailed_from_<YYYY-MM-DD|all>_until_<YYYY-MM-DD|all>.json` in that same private directory and never overwrite full-history results. Summary, transactions and detailed GETs read only that owner's matching bundle; missing/mismatched cached ranges require explicit recalculation.
- CSV/JSON files are published individually by atomic rename, so readers do not receive partially written files. A failed job preserves its previous last-success timestamp; publication across multiple files and the history database is not a single transaction.

### How Doughbox calculates returns

All return calculations use decimal EUR amounts. Monetary aggregates are rounded at presentation, not on every transaction to retain precision.

**Economic return:** securities use fee-inclusive EUR moving-average cost; sales realize net proceeds less proportional remaining cost. `performance = realized trading P&L + unrealized P&L + recorded gross EUR dividends`; `total_return = 100 × performance / cumulative gross EUR purchase spend`, including purchase fees. Sales and reopening a position do not reduce that denominator. Signed dividend corrections reverse income, while purchase basis and gross spend are unchanged. This is a simple cumulative return including recorded income, **not annualized, TWR or IRR**. It is before withholding tax, without projected income or deemed fund distributions. Positions without a positive purchase baseline retain their income but display percentage return as unavailable.

**Portfolio and history:** whole-portfolio P&L additionally includes recorded interest. Interest linked to a custom cash/debt holding is counted through that holding only; unlinked interest is added once at portfolio level, independently of tax treatment. `PortfolioOverview.income` exposes the recorded income total; its `realized` includes realized trading/custom returns and income, while individual securities `realized` remains trading-only with a separate `income` column. `PortfolioOverview.total_invested` is the gross denominator. Historical `PerformanceSignal.total_invested` retains its **net capital baseline** meaning (`value − economic P&L`). V17 leaves unknown legacy gross spend `NULL`; V18 stores new income-inclusive holding-level `components` without fabricating older breakdowns or backfilling income.

The **Active only** switch at the top of Performance applies to history, the conviction scatter, the individual table and visible alpha. It selects today's positive-unit security and manual holdings. Filtered history sums their saved component values, net baselines and gross contributions before calculating a percentage; it is the history of today's selected assets, not the set of holdings active on each historical date. Closed holdings and unlinked interest are excluded. Incomplete acquisition history makes the active selection unknown and hides every filtered widget until reconciliation.

Dividend reversals carry signed taxable income, recorded withholding, estimated credits and provisional tax adjustments. Annual dividend tax is floored at zero only after those adjustments are summed, so a fully reversed payment does not leave tax owed on zero income.

Benchmark `simulated` and `alpha` fields are `null` when no FRED series is configured; absent comparison data is not zero benchmark performance or outperformance.

Paths below are relative to `core/src/`, except `web/` paths.

| Calculation                              | Implementation                                                                                                                                      | Formula / boundary                                                                                                                                                                                                                                                                                              |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Imported trade grouping                  | `services/performance.rs::load_trade_groups`                                                                                                        | Canonical ISIN + broker; all renamed-identifier history joins one pool, even if source rows are nonadjacent.                                                                                                                                                                                                    |
| Economic security WAC / realization      | `performance.rs::get_title_performance`                                                                                                             | Buy basis/spend adds `units × EUR price + EUR fees`; sales remove proportional WAC and realize `units × EUR price − sale fees − removed basis`. Incomplete sell history errors.                                                                                                                                 |
| Live/exported security returns           | `performance.rs::compute_performance`, `get_performance`                                                                                            | Unrealized = market value − remaining basis; add recorded gross EUR dividends through the cutoff, including signed corrections. Merge broker EUR amounts by canonical ISIN before calculating percentages. Trading realized remains separate from income; API and CSV/JSON share the computation.               |
| Buy-in display                           | `performance.rs::get_buy_ins`                                                                                                                       | Remaining fee-inclusive economic EUR WAC, current units/value, unrealized P&L and cumulative gross spend per broker/ISIN. **Not the fee-exclusive tax WAC.**                                                                                                                                                    |
| Portfolio value/allocation               | `services/portfolio.rs::get_portfolio_overview`                                                                                                     | Sum known security/custom values; allocation = holding value / known portfolio value. Missing security prices set `valuation_complete=false`.                                                                                                                                                                   |
| Portfolio economic P&L                   | `portfolio.rs`, `performance.rs::security_costs`, `database/queries/interest.rs::unlinked_interest_total`                                           | Sum security trading P&L plus dividends, known custom P&L including linked income, and unlinked recorded interest once. Denominator is cumulative gross purchases/contributions, not income. Missing valuations/cost data remain explicitly incomplete.                                                         |
| Gold/property                            | `services/assets.rs::calculate_trade_holding`                                                                                                       | Fee-inclusive economic WAC; realized net sale proceeds − proportional basis; unrealized latest recorded EUR valuation × units − basis. Cumulative gross purchases remain unchanged by sales.                                                                                                                    |
| Cash accounts                            | `assets.rs::fold_cash_events`, `calculate_cash_holding`                                                                                             | P&L = current EUR balance − net external contributions; realized = credited income + realized economic FX; unrealized is the remainder. Withdrawals remove proportional EUR basis.                                                                                                                              |
| Cash snapshots                           | `assets.rs::fold_cash_events`                                                                                                                       | Opening replaces earlier balances/contributions/income/realized FX. Reconciliation treats unexplained native changes as external capital, preserving FX return; not taxable income or a complete tax cash ledger.                                                                                               |
| Private debt                             | `assets.rs::calculate_debt_holding`                                                                                                                 | Current EUR principal + recorded income − net advances; repayments remove proportional basis and realize EUR FX. No projected income, defaults or automatic impairment.                                                                                                                                         |
| Custom aggregation                       | `assets.rs::summarize_custom_holdings`                                                                                                              | Sum known values/returns and their gross contributions separately; unavailable valuation/cost increments incomplete count.                                                                                                                                                                                      |
| Weighted cash rate                       | `assets.rs::weighted_cash_interest_summary`                                                                                                         | `Σ(positive converted EUR balance × annual rate%) / Σ(eligible EUR balances)`. No automatic accrual; unavailable conversions excluded and counted.                                                                                                                                                              |
| Manual EUR amounts/fees                  | `assets.rs::resolve_eur`, `resolve_trade_fees_eur`, `try_convert_current`                                                                           | Explicit historical EUR equivalent or date FX; fee conversion may use explicit EUR/native price ratio. Missing current FX makes valuation unavailable.                                                                                                                                                          |
| Position quantities                      | `database/queries/position.rs::get_positions`, `services/positions.rs`                                                                              | Canonical-ISIN purchases minus sales, split-adjusted only through the requested date; name/sort enrichment does not recalculate returns.                                                                                                                                                                        |
| Stock splits                             | `services/instruments/stock_splits.rs`                                                                                                              | Compound all applicable `to/from` unit factors; price uses inverse factors, preserving consideration.                                                                                                                                                                                                           |
| EUR FX / GBX                             | `database/queries/fx_rate.rs`, `services/market_data/fx_rates.rs::convert_amount`                                                                   | Accounting conversion retains its EUR-leg requirement. MiFID quote conversion additionally supports stored EUR-cross rates. GBX applies 100 pence/GBP. Each ECB observation is the latest available **on or before** the reference date, with its actual date retained.                                         |
| Price selection                          | `database/queries/price.rs`, `services/market_data/prices/*`                                                                                        | Positive eligible EUR quotation/manual pin selection; stale pre-split prices cannot silently value current quantities. Feed parsers normalize quotation/notation, not investment returns.                                                                                                                       |
| Benchmark                                | `performance.rs::simulate_alternate_purchase`, `market_data/fred.rs`                                                                                | Gross EUR purchase budget / nominal index level; sales release the same split-adjusted held fraction. Latest valid observation on/before date; unavailable prehistory errors. A price-index proxy, not an investable EUR total-return benchmark.                                                                |
| Snapshot/reporting                       | `services/portfolio_reports.rs`, `database/{models,queries}/performance.rs`, `services/telegram_reports.rs`                                         | Persist raw income-inclusive value/net/gross aggregates and per-asset components. P&L = value − net baseline; period P&L change = value delta − baseline delta. Reject incomplete snapshots. Legacy component breakdowns remain unknown.                                                                        |
| Monthly activity                         | `database/queries/composite.rs::get_monthly_investment_activity`                                                                                    | UTC monthly buys − sales before fees; cash deposits − withdrawals; recorded dividends + interest. Three separate, non-additive measures; includes archived history, excludes valuations/snapshots/debt principal.                                                                                               |
| Austrian security cost                   | `services/taxation/wac.rs::SecWac::update`                                                                                                          | Moving EUR average per broker/ISIN: `(old units × old EUR WAC + new units × acquisition EUR price) / total units`. No deductible acquisition fees. Broker identifies the brokerage portfolio; multiple brokerage portfolios at the same broker cannot be distinguished.                                         |
| Austrian security disposal               | `taxation/process.rs::TaxEngine::trade`                                                                                                             | Sale EUR proceeds − sold units × historical EUR WAC. Includes FX while holding the security; excludes trading fees and does not add a second cash-FX gain.                                                                                                                                                      |
| Austrian gross income / withholding      | `services/events.rs::income_tax_values`, `TaxEngine::income`, `services/dtt.rs`                                                                     | Native gross converted independently to EUR; actual withholding kept separate from estimated treaty-capped credit. Never increase gross by only the creditable withholding. Unknown treaty/source does not imply a credit.                                                                                      |
| Reporting funds / OeKB                   | `TaxEngine::fund`, `normalize_fund_shares`, `market_data/oekb.rs`                                                                                   | 10287 supplies dividend-equivalent income; 10289 adjusts EUR basis; 10288 supplies reported assessment credit subject to supported context. Normalize per-share reports for splits. 10595 is a warning and 10759 loss-offset information, **not extra income**.                                                 |
| Annual / detailed / transaction taxation | `taxation/process.rs::calculate_report`, `taxation/impacts.rs`                                                                                      | One historical replay; pre-range transactions establish basis; selected dates count income; end date inclusive. Sum unrounded transaction amounts; annual presentation rounds once. Signed gains/losses remain distinct; liabilities are provisional before full annual assessment.                             |
| Separate cash FX taxation                | `TaxEngine::process`, `taxation/types.rs`                                                                                                           | `fx_appreciation=null`, no currency WAC emitted: a complete cash ledger, opening EUR basis, account classification and held-versus-automatic FX identification are absent. Manual calculation required; null is **not zero taxable gain**.                                                                      |
| Import normalization                     | `services/importers/{lightyear,trading212,ibkr,revolut,wise,manual,erste_bank,scalable}.rs`, `trade_republic/{csv,pdf}.rs`                          | Record execution EUR equivalents, units, cash legs, fees and withholding. Imported `Trade.fees` contract is EUR; Lightyear/T212/IBKR normalize fee currency. T212 normalizes non-EUR account totals and gross dividends; Revolut no longer invents 15% withholding. Legacy data requires source reconciliation. |
| Timeline/event display                   | `services/events.rs`, `api/handlers/timeline.rs`, `web/src/components/composite/event-card.tsx`                                                     | Recorded quantities/prices/totals and explicit currencies; no second tax or return engine. Snapshots are baselines/reconciliation, not income.                                                                                                                                                                  |
| Frontend return charts                   | `web/src/components/charts/{portfolio,position}-performance-chart.tsx`, `web/src/pages/dashboard/performance.tsx`                                   | Shared Active only selects today's positive holdings for every widget. History calculates a ratio of summed matching components; securities chart/table include recorded dividend income. Missing breakdowns/gross baselines remain gaps, and unknown active selection hides every widget.                      |
| Frontend holdings/tax/activity           | `web/src/pages/dashboard/{portfolio,positions,assets,taxation,timeline,instruments}.tsx`, `web/src/components/charts/investment-activity-chart.tsx` | Format authoritative backend decimals; no local averaging/summing of return percentages. Tax UI distinguishes recorded withholding, estimated credits and unavailable cash FX.                                                                                                                                  |
| Formatting/visual scale                  | `services/shared/util.rs`, `web/src/lib/utils.ts`, `web/src/components/composite/bar-list.tsx`                                                      | Monetary/percentage display rounding and sign colors; relative bar widths are a visual scale, not portfolio allocation. No accrual/projection math in settings or status controls.                                                                                                                              |

### Austrian tax interpretation and sources

Security FX and cash FX are **different assets and realization events**. Spending previously held USD to buy a security can realize a separate cash gain. The security then starts with its acquisition-date EUR value; selling it realizes its full EUR gain even when proceeds remain USD.

Doughbox does **not** infer missing cash acquisition costs, multiple brokerage portfolios, historical income conventions, treaty eligibility, fund corrections/distribution reconciliation, or final cross-category annual loss offsets. The standard 25% bank-deposit interest and 27.5% securities/dividend rates are estimates where source classification is incomplete.

- [EStG §27a, consolidated statute](https://ris.bka.gv.at/NormDokument.wxe?Abfrage=Bundesnormen&Gesetzesnummer=10004570&Paragraf=27a): special rates and moving-average EUR acquisition costs.
- [BMF: realized capital gains](https://www.bmf.gv.at/themen/steuern/sparen-veranlagen/substanzgewinne-bzw-einkuenfte-aus-realisierten-wertsteigerungen.html): proceeds less acquisition costs; acquisition/transaction/brokerage portfolio expenses are not deductible under the special-rate private-investor rules.
- [BFG RV/7106221/2019](https://findok.bmf.gv.at/findok/iwg/129/129828/129828.1.pdf), pp. 3–4 and 9–11: separate cash disposal when buying securities, EUR security basis/proceeds and embedded FX.
- [BMF: capital income](https://www.bmf.gv.at/themen/steuern/sparen-veranlagen/kapitalertraege-im-engeren-sinn.html): interest/dividend taxation and foreign withholding.
- [OeKB official tax-field definitions](<https://www.oekb.at/dam/jcr:5a83a4b7-c695-45ab-ad87-2de5b811a580/Feldliste_Steuerdaten_Fonds_(gesamt)_2025-11-28.pdf>), especially p. 10: reporting-fund income, credits, acquisition-cost corrections and warning fields.

### Investment activity

The **Timeline** chart shows three grouped, separately colored monthly series:

- **Net purchases:** imported security buys and manual asset buys, minus sales, using their recorded EUR trade amounts before fees.
- **Net cash added:** cash-account deposits minus withdrawals, using their recorded EUR amounts. Historical flows remain included when an account or holding is archived.
- **Dividends & interest:** recorded dividends plus imported and manually recorded interest, including cash, share-lending, and private-debt interest. Uses gross recorded EUR amounts before withholding tax, not projected accruals. Linked imported interest is counted once, regardless of tax treatment.

Valuation changes, opening/reconciliation balances, and private-loan principal advances or repayments are excluded. Purchases and cash flows remain signed: negative values mean net sales or cash withdrawn. Historical income remains included for archived holdings. The series share one EUR scale and are not stacked or combined into a total.

`GET /api/timeline/investment-activity` returns `month`, `net_purchases_eur`, `net_cash_added_eur`, and `dividends_interest_eur`. Amounts are decimal strings, summed across ledgers and then rounded to two decimals. Transactions are grouped by UTC month without recalculating historical FX rates. Income-only months are included. The timeline **Start Date** control filters both the chart and event list from the selected day, inclusive at midnight UTC. The chart filters transactions before aggregation, so the first month's totals exclude activity before that day. API callers can pass `?start_date=YYYY-MM-DD`; omitting it returns all recorded investment activity.

### Manual holdings

**Portfolio** has **Summary**, **Holdings**, and **Timeline** sections. Under **Holdings**, **Imported** lists securities imported from broker statements or CSV, identified by ISIN and valued using available quotes or manual price pins; **Manual** contains custom assets without an ISIN, with manually recorded activity and valuations and no automatic price feeds. Imported opens by default. The selected holdings subtab and its filters are retained when switching between portfolio sections. Summary and Timeline combine both groups.

Supported manual holdings:

- Physical gold and real estate: buys, sells, and manual valuations.
- Private debt: principal advances, repayments, and actual interest.
- Cash accounts: deposits, withdrawals, opening/reconciliation balances, and a current gross annual rate.

Cash rates never accrue automatically; record interest when it is credited. An opening balance is the performance baseline. Reconciliation is capital-neutral and keeps FX return. Custom activity is excluded from automated Austrian taxation. Imported interest can be linked without changing its tax treatment; manually created interest is stored as excluded. Archive requires a zero current balance.

Example:

```bash
curl -H "Authorization: Bearer $DOUGHBOX_PERSONAL_READ_TOKEN" \
  "http://localhost:8084/api/trades?isin=US0378331005&broker=Trade%20Republic"
```

### Local development

You need Rust 1.97.1 (see `rust-toolchain.toml`), pnpm 10, Node 22.12+, typeshare CLI and PostgreSQL 15+.

1. Configure `core/.env.dev` with `POSTGRES_URL`, persistent `AUTH_MASTER_KEY`, and `DOUGHBOX_PUBLIC_URL` matching the browser's Vite origin (normally `http://localhost:5173`), plus explicit `AUTH_ALLOW_INSECURE_HTTP=true` for local HTTP. A missing or invalid origin/key fails `pnpm dev` before the API serves requests. Never reuse production portfolio/output data for smoke tests.
2. From the repo root: `pnpm install`
3. `pnpm dev` starts the API (`cargo watch` on `:8084`) and same-origin Vite proxy at `http://localhost:5173`. The API requires the database configured by `POSTGRES_URL` to be running. From `core`, run `cargo run -- users bootstrap` with the same configuration to set up the first account; subsequent financial CLI commands need explicit `--user`.
4. After changing Rust types marked with `#[typeshare]`, run `pnpm typeshare` to regenerate `web/src/types/core.ts`.
5. `pnpm test` runs `cargo test`. `pnpm typecheck` runs `tsc --noEmit`. `pnpm core lint` runs clippy.
   To match CI's Rust lint coverage, run `cargo clippy --all-targets` from `core`; this checks test code as well as the application.

[Agentation](https://www.agentation.com/install) is available in the web app during local development. Open the floating toolbar to annotate UI elements and copy feedback into your coding agent. It runs locally without an MCP server or external endpoint and is excluded from production builds.

UI icons use [Phosphor](https://phosphoricons.com/) through `@phosphor-icons/react`, including shared controls. Use canonical `*Icon` exports with the default regular weight and Tailwind `text-*` colors (`currentColor`), not `stroke-*` colors. `web/components.json` selects Phosphor for generated components.

Frontend tooling uses platform-specific optional dependencies. The workspace retains macOS and Linux bindings for ARM64 and x64 so installing inside Linux does not prune macOS bindings from a shared checkout. If Vite reports a missing Rolldown native binding, run these commands in a **shell**, not a Python cell:

```sh
pnpm install --frozen-lockfile
pnpm dev
```

Keep `pnpm-lock.yaml`; no `npm install`, lockfile deletion or WASM fallback is needed.

For dependency refreshes, run `pnpm --recursive update --latest` from the root so both root tooling and web packages are included. Update Rust requirements in `core/Cargo.toml` for new major releases, then run `cargo update` from `core` to refresh transitive dependencies. Keep both lockfiles and verify with `pnpm test`, `pnpm typecheck`, and `pnpm web build:docker`.

The complete isolated database suite runs in CI and is opt-in locally. From `core`, export a dedicated PostgreSQL 17 `TEST_POSTGRES_URL`, then run `cargo test -- --ignored --skip pricing_live_provider_smoke --test-threads=1`. This includes auth, tenant isolation, migration, pricing/streaming, calculation, Telegram, import, income, FX and trading regressions. Tests create/drop unique schemas and never fall back to `POSTGRES_URL` or dotenv databases. Single-threading avoids database-wide advisory-lock fixtures interfering across schemas; the skipped internet/live-provider smoke is optional.

Authentication regressions read persisted creation timestamps directly to verify challenge, full-session, invitation, and recovery-link lifetimes.

### Database migrations

Applied migrations must retain their original version, name and contents: Refinery validates `refinery_schema_history` at startup. V1–V32 remain unchanged; V33 adds account ownership and trigger-safe legacy backfill. PostgreSQL below 15 is rejected before its owner-preserving interest foreign key is applied. Always stop old writers and follow the backup/bootstrap upgrade procedure above; no legacy-owner/login compatibility fallback exists.

## Playbooks

### Trade Republic bulk import

Trade Republic doesn't support any bulk export or direct, documented API access currently. Luckily, there's a great tool to bulk download all of your statements: https://github.com/pytr-org/pytr. Once you've downloaded all of your statements, you can then import the PDF files (e.g. in `Abrechnung` or `Sparplan`) by copying them into the `input` folder of Doughbox.

### Interactive Brokers Trade Export via Flex Query

To import your Interactive Brokers trades into Doughbox, set up a Flex Query: log into IBKR, click "Performance & Reports" > "Flex Queries" > "Create", Select "Trades" under sections, then "Select All". Choose "CSV" under "Format". Give the query a name, tap "Next", then review and "Confirm".

### Manual imports

For manual imports of trades, you can create a CSV with the following structure, place it in the `input` folder and import it:

| date                    | broker | ISIN           | avg_price_per_unit | eur_avg_price_per_unit | units | direction   | security_type                | currency           | fees | withholding_tax | withholding_tax_currency |
| ----------------------- | ------ | -------------- | ------------------ | ---------------------- | ----- | ----------- | ---------------------------- | ------------------ | ---- | --------------- | ------------------------ |
| 2019-10-29 14:37:45.123 | VSOP   | USXXXXXXXXXXXX | 10.231             | 9.123                  | 10    | Buy \| Sell | Equity \| Bond \| Derivative | EUR \| USD \| .... | 0.0  | 0               | EUR \| USD \| ...        |

## Acknowledgements

As Carl Sagan put it: "If you want to bake an Apple Pie, you must first invent the universe from scratch". In that spirit, Doughbox simply cobbles together a bunch of excellent open source libraries and existing tools. Here's a non-exhaustive list:

- Rust, JavaScript & React
- shadcn, an excellent UI Library built on top of Tailwind
- Tokio and axum
- Postgres
- SWR
- Vite

## Disclaimer

I don't take responsibility for any tax implications, financial losses or similar as a result of using Doughbox. Please check the code and do your own research before using it.
