# Doughbox

## What is this?

Doughbox is a self-hosted portfolio tracker. It lets you import various brokerage statements (more on supported brokers below), stores them all in a unified format in a Postgres instance and then displays various stats on your portfolio, either via CLI or web interface (which can also be used as PWA on your phone).

If you need to take care of capital gains taxes in Austria, you can use Doughbox to help you prepare your tax declaration. Beware that I'm not a tax advisor and I take no responsibility for the outcome — you're responsible for your taxes, so: read the code, do your own research and consult a professional tax advisor.

<img alt="Screenshots of Doughbox" src="./.github/doughbox-screenshots.png" style="width: 100%" />

## Features

- Self-hosted, with a Docker image for easy deployment.
- Web interface, CLI and API.
- Support for FX conversions and trades in non-Euro currencies with FX rates from the ECB.
- Optional automatic EUR equity/ETF prices from Xetra, Tradegate, Frankfurt, Cboe Europe and Euronext, using a durable Postgres queue with scheduling and worker limits managed in the web UI.
- Import brokerage statements from various brokers, e.g. Trade Republic, Revolut, Trading 212, etc.
- Display Portfolio Allocation across brokerage accounts.
- Display returns over time for the portfolio and individual positions (both open and closed).
- Display a consolidated timeline across brokers of past trades, interest & dividend payments and fx conversions.
- Go back in time and view your past portfolio's holdings.
- If a FRED (Federal Reserve Economic Data of the St. Louis Federal Reserve Bank) token is set, it also gives you a naive (e.g. no fees or taxes considered) benchmark against the S&P 500.
- Fetches stock split data from polygon.io (hence, you will need an API token from Polygon).
- (Naive, optional) password authentication for login to the web interface. Multi-user support is not planned for now.
- Each transaction is assigned an ID either based on the ID from the broker or, if that isn't available, a combination of transaction characteristics. Thus, reimporting the same brokerage statement will not cause duplicate entries.
- It can give you a capitals gains overview, using perpetual WAC and separating FX gains/losses from capital gains/losses with Austrian capital gains tax rates (i.e. 25% for cash interest, 27.5% for capital gains at the time of writing).
- Set `TG_TOKEN` and `TG_CHAT_ID` to receive daily Telegram portfolio summaries on a schedule configured in **Settings**, or send one manually with `portfolio -n`.

## (Known) Limitations

- EUR as base currency — it supports trades in other currencies, but the base currency needs to be EUR.
- Automatic prices are opt-in and currently cover EUR monetary quotations for instruments imported as `Equity` (including ETFs). Bonds, derivatives and non-EUR market quotations are not automatically priced. Coverage depends on eligible post-trade reports on the enabled venues, including zero-volume price indications; these are delayed valuations, not guaranteed executed-trade prices, live quotes or official closing prices.
- Doughbox uses ISINs as primary identifier, so it's agnostic to the trading venue an instrument was bought. If it encounters a ticker symbol during the import, it attempts to convert it to an ISIN via the OpenFIGI API
- Crypto isn't supported.
- Custom physical gold, real estate, private loans, and interest-bearing cash accounts can be tracked manually in the web UI. They aren't considered for Taxation.
- Housekeeping, precomputed performance, and taxation run from the CLI. The web interface imports brokerage statements from **Portfolio → Timeline** and manages custom assets, price feeds and scheduling, worker concurrency, log verbosity, existing instrument names, and manual EUR prices.

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

All feeds use eligible EUR monetary reports for known equity/ETF ISINs. Select exchanges in **Data → Price Feeds → Manage scheduler**; refresh and reset controls are available per exchange. These are delayed valuations, not live quotes or guaranteed closing prices. See [Fetch current prices](#fetch-current-prices) for availability, prioritization and provider terms. gettex and EIX remain deferred pending verified post-trade formats and correction semantics.

## Why does this exist?

Doughbox is scratching my own itch: I have lots of different brokerage accounts and I wanted an easy way to stay on top of my portfolio allocation and its performance. While there are great portfolio tracking apps out there (Delta, Finanzfluss Co-Pilot, Parqet, Quin), I wanted to have full control over the data and the supported brokers. And when the need for some custom visulations and capital gains tax preparation came about, I started developing Doughbox. Since I use a lot of open source, self-hosted services, it makes sense for me to open source my personal infrastructure, too.

## Get Started

In order for Doughbox to run, you need a running Postgres instance. A polygon.io API key is needed for stock-split housekeeping, but not for the API or price refreshes.

### Environment variables

| Variable                  | Required         | Purpose                                                                        |
| ------------------------- | ---------------- | ------------------------------------------------------------------------------ |
| `POSTGRES_URL`            | yes              | Postgres connection string                                                     |
| `POLYGON_TOKEN`           | for stock splits | Stock-split data from polygon.io                                               |
| `FRED_TOKEN`              | no               | S&P 500 benchmark via FRED                                                     |
| `PASSWORD`                | no               | Web UI login password                                                          |
| `API_TOKEN`               | no               | Bearer token for the HTTP API                                                  |
| `TG_TOKEN` / `TG_CHAT_ID` | no               | Scheduled Telegram portfolio reports and manual notifications (`portfolio -n`) |
| `RUST_ENV`                | no               | Loads `.env.dev` when `development` (default), `.env.prod` when `production`   |

The **Settings** page shows whether `TG_TOKEN` and `TG_CHAT_ID` are set on the server. Set values use the same green check-circle badges as successful imports; missing, empty, or whitespace-only values use a muted **Not set** badge. This read-only status never exposes the values and does not validate them with Telegram.

### Docker

```bash
  docker run -d \
  -e POSTGRES_URL="your_postgres_url_here" \
  -e POLYGON_TOKEN="your_polygon_token_here" \
  -p 8084:8084 \
  -v /path/to/your/input-directory:/app/input \
  -v /path/to/your/output-directory:/app/output \
  ghcr.io/alxjpzmn/doughbox:latest
```

Once the app runs, the Web UI will be available. Interactive API docs are at `/api/docs` (OpenAPI spec at `/api/openapi.json`). If you've also set `API_TOKEN`, you can call the API with `Authorization: Bearer $API_TOKEN`.

First, import your brokerage statements (PDF or CSV, depending on the broker) using **Portfolio → Timeline → Import**. Drop multiple files into the upload area or use **Choose files**, then select **Upload and queue**. Alternatively, place the statements in a folder and run `./doughbox import foldername`.

If you run Dougbox inside Docker, the directory path needs to correspond to to the path inside the container, so `./input`. The full command for importing then becomes: `docker container exec -i doughbox ./doughbox import ./input`. The first import will likely be a bit slower than subsequent ones, since Doughbox will fetch historic FX rates.

After the first import, you will be prompted to run housekeeping and some portfolio and PL calculations, unless you called the import command with `--silent`. Especially on the first run it's highly recommended to run at least `housekeeping` (which will e.g. fetch data on past stock splits based on your holdings).

The web interface will be available per default on port `8084`.

You can run the following commands in the CLI:

`housekeeping`: for fetching stock split data, tax data for funds and ETFs from the Austrian Control Bank (OeKB) and updated FX rates from the ECB.

`portfolio`: to show your current holdings and their allocation in the terminal. Pass `-n` / `--notify` to also send the summary via Telegram.

`performance`: this takes a while to run and calculates your return for the portfolio as such plus each present and past position within it. It also benchmarks it against the S&P 500 if you've set a FRED API Token. Once you've run it, it will also be shown in the web interface.

`taxation`: also takes a while to run and will output your realized gains and losses, dividends, interest etc. using perpetual WAC. It will also display dividend aequivalents using OeKB data and withheld tax from brokerage statements. Optional flags: `--from YYYY-MM-DD`, `--until YYYY-MM-DD`, `--with-events` (per-transaction breakdown).

`debug-pdf <path>`: print extracted text from a PDF statement (useful when adding or debugging an importer).

`api`: the command to run the web server that serves both the web frontend and the API. The docker container automatically runs this.

`prices refresh [--feed xetra|tradegate|frankfurt|cboe|euronext] [--dry-run]`: enqueue the persisted feed selection (or one explicitly named feed) and wait for completion, or validate the available last 48 hours without writing prices/checkpoints/jobs. Shows queue status and refresh progress; does not prompt for FX updates. With no selected feeds, choose a feed explicitly or update the UI selection.

`prices status`: JSON summary of the schedule, queue and current holdings' price sources/timestamps.

`prices worker`: run queue consumers for all supported feeds without the HTTP server or scheduler. Useful for a separate worker container; automatic scheduling requires an `api` process and the persisted scheduler enabled. Each process applies the worker limit saved in the web UI.

`prices set <ISIN> --price 123.45 [--as-of 2026-10-15T12:00:00Z]`: pin a positive manual **EUR per-unit** price. `prices unpin <ISIN>` removes the override and reselects stored market data or an execution fallback.

### Telegram portfolio reports

Open **Settings → Telegram notifications** to enable **Daily portfolio reports**, choose the **Daily send time (UTC)**, and **Save report schedule**. The schedule starts paused with **18:00 UTC** preselected. It persists across restarts and applies without restarting; the API must be running, but the browser need not be open.

### Brokerage statement imports

Open **Portfolio → Timeline** and tap **Import** to import brokerage transactions for supported brokers. Drag one or more PDF/CSV files into the drop area, or use **Choose files**, then select **Upload and queue**. The drop area highlights while files are dragged over it. **Hide imports** collapses the section without clearing selected files or stopping queue updates. Brokers are detected automatically. Uploads accept up to 100 non-empty files per batch, 25 MiB per file, and 99 MiB combined; the API allows a 100 MiB multipart body including headers. Invalid uploads reject the entire batch before enqueueing any file. Keep the page open until upload confirmation; afterwards the API processes the files without the browser.

API clients can use `POST /api/statement-imports` with repeated multipart fields named `files`, then poll `GET /api/statement-imports`. Both return `{"jobs":[...]}`; POST returns `202` after the whole batch is persisted. Job IDs are strings; each job includes its filename, byte size, status, queued/started/finished timestamps, and error when failed.

```bash
curl -H "Authorization: Bearer $API_TOKEN" \
  -F 'files=@statement.csv' -F 'files=@statement.pdf' \
  http://localhost:8084/api/statement-imports
```

### REST API

The API provides portfolio/tax reads, custom-asset management, instrument name/manual-price management, scheduler/import controls, and non-sensitive application settings. Browser sessions from `POST /api/login` and `Authorization: Bearer $API_TOKEN` are both accepted; all writes use the same authentication.

Sessions are stored in memory and are lost when the API restarts; separate API replicas do not share session records. A browser retaining an old cookie can then produce `record not found in store` warnings and must sign in again. These session lookups are independent of price-worker jobs.

| Method               | Path                                                          | Purpose                                                                                                                                                               |
| -------------------- | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`                | `/api/docs`                                                   | Interactive Swagger UI (no auth)                                                                                                                                      |
| `GET`                | `/api/openapi.json`                                           | OpenAPI spec                                                                                                                                                          |
| `GET`                | `/api/portfolio`                                              | Allocations and total return                                                                                                                                          |
| `GET`                | `/api/positions?date=&isin=&broker=`                          | Holdings as of a date                                                                                                                                                 |
| `GET`                | `/api/trades?isin=&broker=&direction=&from_date=&until_date=` | Trades                                                                                                                                                                |
| `GET`                | `/api/buy-ins?isin=&broker=`                                  | WAC cost basis per instrument and broker                                                                                                                              |
| `GET`                | `/api/performance?isin=&broker=`                              | Live performance (does not write files)                                                                                                                               |
| `GET`                | `/api/performance_overview`                                   | Precomputed `output/performance.json`                                                                                                                                 |
| `GET`                | `/api/past_performance`                                       | Historical value snapshots                                                                                                                                            |
| `GET`                | `/api/timeline?start_date=&isin=&broker=&event_type=`         | Unified event timeline                                                                                                                                                |
| `GET`                | `/api/timeline/investment-activity?start_date=`               | Monthly net purchases, net cash added, and dividends & interest (EUR), optionally filtered from an inclusive UTC start date                                           |
| `GET`                | `/api/statement-imports`                                      | All active and latest 100 completed statement uploads with per-file timestamps and errors; newest ID first                                                            |
| `POST`               | `/api/statement-imports`                                      | Atomically queue repeated multipart `files` fields (PDF/CSV); returns `202` with accepted jobs; 100 files, 25 MiB each, 100 MiB total request                         |
| `GET`                | `/api/instruments` / `/api/instruments/{isin}`                | All known instruments with safe `quote` valuation/provenance metadata; existing top-level fields remain available                                                     |
| `PATCH`              | `/api/instruments/{isin}`                                     | Rename an existing instrument with `{"name":"Display name"}`; trims/rejects blank names without changing prices                                                       |
| `PUT`                | `/api/instruments/{isin}/price`                               | Pin a positive manual EUR unit price, e.g. `{"price":"123.456789","as_of":"2026-10-18T12:00:00Z"}`; `as_of` is optional and cannot be in the future                   |
| `DELETE`             | `/api/instruments/{isin}/price`                               | Remove the manual pin and reselect stored market data or fallback                                                                                                     |
| `GET`                | `/api/prices`                                                 | Price schedule, queue errors and current holdings' quote provenance (no upstream requests)                                                                            |
| `PUT`                | `/api/prices/scheduler`                                       | Persist `{"enabled":true,"interval_minutes":30,"worker_concurrency":2,"feeds":["DETR-posttrade","DFRA-posttrade"]}`; apply without restart                            |
| `POST`               | `/api/prices/refresh`                                         | Queue the selected feeds, even while automatic scheduling is paused; returns `202` with current status                                                                |
| `POST`               | `/api/prices/feeds/{feed}/refresh`                            | Queue one supported feed; preserves active work and retry backoff                                                                                                     |
| `POST`               | `/api/prices/feeds/{feed}/reset`                              | Retire a queued/running job and queue a fresh attempt; preserve completed checkpoints, reports, prices, pins and history                                              |
| `GET`                | `/api/data-imports`                                           | Polygon/ECB availability, persisted per-source schedules, next runs, import status, timestamps, errors, stored counts and latest dates                                |
| `POST`               | `/api/data-imports/{source}/refresh`                          | Start `polygon` or `ecb` in the background; returns `202` with status, or `503` when the Polygon token is missing                                                     |
| `PUT`                | `/api/data-imports/{source}/scheduler`                        | Persist `{"enabled":true,"frequency":"monthly"}` for `polygon` or `ecb`; frequencies: `daily`, `weekly`, `monthly`; returns all source statuses                       |
| `GET` / `PUT`        | `/api/settings`                                               | Read/save non-sensitive preferences, e.g. `{"log_level":"DEBUG"}`; responses include read-only `telegram_token_set` / `telegram_chat_id_set` flags, never credentials |
| `PUT`                | `/api/settings/telegram-scheduler`                            | Save daily Telegram reports with `{"enabled":true,"send_time_utc":"18:00"}` (strict 24-hour UTC); returns settings including persisted `telegram_schedule` status     |
| `GET`                | `/api/brokers`                                                | Distinct broker names (use these as filter values)                                                                                                                    |
| `GET`                | `/api/dividends` / `/api/interest` / `/api/fx-conversions`    | Cash events                                                                                                                                                           |
| `GET`                | `/api/taxation`                                               | Tax report                                                                                                                                                            |
| `GET`/`POST`         | `/api/assets`                                                 | List or create custom assets (`?include_archived=true` to include archived)                                                                                           |
| `GET`/`PUT`/`DELETE` | `/api/assets/{asset_id}`                                      | Custom asset detail, update, and archive                                                                                                                              |
| `POST`               | `/api/assets/{asset_id}/restore`                              | Restore an archived custom asset                                                                                                                                      |
| `GET`                | `/api/assets/holdings`                                        | Custom holdings and EUR values                                                                                                                                        |
| `GET`                | `/api/cash-accounts/summary`                                  | Volume-weighted cash interest                                                                                                                                         |

Portfolio responses combine securities and custom holdings. Missing security quotes have `value: null` and `share: null`, set `valuation_complete: false`, and are listed in `unpriced_isins`; aggregate returns must not be used in that state. Custom holdings with missing valuation or cost data set `returns_incomplete: true` and increment `unpriced_assets`; only known returns are included. The portfolio summary shows these conditions separately. The CLI refuses to save a performance snapshot or send notifications when either condition is present.

### Investment activity

The **Timeline** chart shows three grouped, separately colored monthly series:

- **Net purchases:** imported security buys and manual asset buys, minus sales, using their recorded EUR trade amounts before fees.
- **Net cash added:** cash-account deposits minus withdrawals, using their recorded EUR amounts. Historical flows remain included when an account or holding is archived.
- **Dividends & interest:** recorded dividends plus imported and manually recorded interest, including cash, share-lending, and private-debt interest. Uses gross recorded EUR amounts before withholding tax, not projected accruals. Linked imported interest is counted once, regardless of tax treatment.

Valuation changes, opening/reconciliation balances, and private-loan principal advances or repayments are excluded. Purchases and cash flows remain signed: negative values mean net sales or cash withdrawn. Historical income remains included for archived holdings. The series share one EUR scale and are not stacked or combined into a total.

`GET /api/timeline/investment-activity` returns `month`, `net_purchases_eur`, `net_cash_added_eur`, and `dividends_interest_eur`. Amounts are decimal strings, summed across ledgers and then rounded to two decimals. Transactions are grouped by UTC month without recalculating historical FX rates. Income-only months are included. The timeline **Start Date** control filters both the chart and event list from the selected day, inclusive at midnight UTC. The chart filters transactions before aggregation, so the first month's totals exclude activity before that day. API callers can pass `?start_date=YYYY-MM-DD`; omitting it returns all recorded investment activity.

The chart has a compact EUR axis, full month/year tooltips with exact EUR values, and a wrapping legend. Dense histories scroll horizontally to preserve readable month and bar widths; the legend and tooltip remain visible outside the scroller. Tiny nonzero amounts have a minimum visible bar height, while true zero values remain empty.

### Manual holdings

**Portfolio** has **Summary**, **Holdings**, and **Timeline** sections. Under **Holdings**, **Imported** lists securities imported from broker statements or CSV, identified by ISIN and valued using available quotes or manual price pins; **Manual** contains custom assets without an ISIN, with manually recorded activity and valuations and no automatic price feeds. Imported opens by default. The selected holdings subtab and its filters are retained when switching between portfolio sections. Summary and Timeline combine both groups.

Supported manual holdings:

- Physical gold and real estate: buys, sells, and manual valuations.
- Private debt: principal advances, repayments, and actual interest.
- Cash accounts: deposits, withdrawals, opening/reconciliation balances, and a current gross annual rate.

Cash rates never accrue automatically; record interest when it is credited. An opening balance is the performance baseline. Reconciliation is capital-neutral and keeps FX return. Custom activity is excluded from automated Austrian taxation. Imported interest can be linked without changing its tax treatment; manually created interest is stored as excluded. Archive requires a zero current balance.

Example:

```bash
curl -H "Authorization: Bearer $API_TOKEN" \
  "http://localhost:8084/api/trades?isin=US0378331005&broker=Trade%20Republic"
```

### Local development

You need [Rust](https://www.rust-lang.org/tools/install) (see `rust-toolchain.toml`), [pnpm](https://pnpm.io), Node 22.12 or newer, and a running Postgres instance.

1. Put credentials/connection details in `core/.env.dev` (`POSTGRES_URL`, `POLYGON_TOKEN` if using stock-split housekeeping, and any optional credentials above). Configure application behavior in the web UI.
2. From the repo root: `pnpm install`
3. `pnpm dev` starts the API (`cargo watch` on `:8084`) and the Vite dev server (proxies `/api` to the API).
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

Queue/price and investment-activity database integration tests are opt-in locally and run in CI. From `core`, run `TEST_POSTGRES_URL=postgres://... cargo test pricing_database -- --ignored` and `TEST_POSTGRES_URL=postgres://... cargo test investment_activity_database -- --ignored`. Tests create and drop their own uniquely named schemas and never fall back to `POSTGRES_URL` or `.env.dev`.

### Database migrations

Applied migration files must keep their original version, name and contents: Refinery validates them against `refinery_schema_history` at startup. V9 is reserved for the historical `custom_assets` migration; automatic pricing is V10. Keep V9 even when its application features are absent so existing databases retain their migration history and asset data.

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
