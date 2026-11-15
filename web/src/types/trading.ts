export type TradingEnvironment = 'demo' | 'live';

export interface TradeResult {
  ticker: string;
  name: string;
  currency: string;
  side: 'buy' | 'sell';
  quantity: string;
  limit_price: string;
  time_validity: 'DAY' | 'GOOD_TILL_CANCEL';
  status: 'pending' | 'submitting' | 'submitted' | 'open' | 'partial' | 'filled' | 'cancelled' | 'rejected' | 'failed' | 'unknown' | 'discarded';
  broker_order_id: string | null;
  error: string | null;
}

interface ProposalEnvelope {
  id: string;
  client_request_id: string;
  title: string;
  note: string;
  environment: TradingEnvironment;
  status: 'pending' | 'executing' | 'completed' | 'discarded';
  created_at: string;
  broker_account?: BrokerAccountState | null;
}

export type Proposal = ProposalEnvelope & (
  { kind: 'trades'; trades: TradeResult[] } | { kind: 'pie'; pie: PieResult }
);

export interface TradingState {
  configured: boolean;
  connection_generation: string;
  environment: TradingEnvironment | null;
  account: Record<string, unknown> | null;
  agent_token_set: boolean;
  mcp_path: string;
  setup_error: string | null;
  proposals: Proposal[];
}

export interface BrokerAccountState {
  fetched_at: string;
  id: string;
  currency: string;
  cash: { available_to_trade: string | null; in_pies: string | null; reserved_for_orders: string | null };
  total_value: string | null;
  positions: Array<{ ticker: string; name: string | null; quantity: string | null; quantity_available_for_trading: string | null; quantity_in_pies: string | null }>;
  notice: string;
}

export type DividendCashAction = 'REINVEST' | 'TO_ACCOUNT_CASH';
export interface PieCreateSettings {
  name: string;
  dividend_cash_action: DividendCashAction;
  instrument_shares: Record<string, string>;
  icon?: string;
  goal?: string;
  end_date?: string;
}
export type PieSettingsChanges = Partial<PieCreateSettings>;
export type PieOperationInput =
  | { action: 'create'; settings: PieCreateSettings }
  | { action: 'update'; pie_id: string; changes: PieSettingsChanges }
  | { action: 'duplicate'; pie_id: string; name: string; icon?: string }
  | { action: 'delete'; pie_id: string };
export interface SubmitPieInput {
  client_request_id: string;
  title: string;
  note: string;
  operation: PieOperationInput;
}
export type PreparedPieMutation =
  | { action: 'create'; settings: PieCreateSettings }
  | { action: 'update'; pie_id: string; settings: PieCreateSettings }
  | { action: 'duplicate'; pie_id: string; name: string; icon: string | null }
  | { action: 'delete'; pie_id: string };
export interface PieSettings {
  name: string | null;
  icon: string | null;
  dividend_cash_action: string | null;
  goal: string | null;
  end_date: string | null;
  instrument_shares: Record<string, string | null> | null;
  instrument_shares_source?: 'expected_share' | null;
}
export interface PieValueResult {
  price_avg_invested_value: string | null;
  price_avg_result: string | null;
  price_avg_result_coef: string | null;
  price_avg_value: string | null;
}
export interface PieSummary {
  id: string;
  cash: string | null;
  dividend_details: { gained: string | null; in_cash: string | null; reinvested: string | null } | null;
  progress: string | null;
  result: PieValueResult | null;
  status: string | null;
}
export interface PieList { fetched_at: string; items: PieSummary[] }
export interface PieInstrument {
  ticker: string | null;
  owned_quantity: string | null;
  current_share: string | null;
  expected_share: string | null;
  issues: { name: string | null; severity: string | null }[] | null;
  result: PieValueResult | null;
}
export interface PieDetail {
  fetched_at: string;
  id: string;
  settings: PieSettings | null;
  instruments: PieInstrument[] | null;
}
export interface PieResult {
  mutation: PreparedPieMutation;
  account_currency: string | null;
  before: PieDetail | null;
  expected_settings: PieSettings | null;
  status: 'pending' | 'submitting' | 'submitted' | 'confirmed' | 'conflict' | 'failed' | 'unknown' | 'discarded';
  broker_pie_id: string | null;
  error: string | null;
  submitted_at: string | null;
  observation: { observed_at: string; state: 'present' | 'absent'; pie: PieDetail | null } | null;
  empty_confirmed_by_owner: boolean;
}
export interface PieApprovalInput { confirm_empty_pie: boolean }
export interface TradingInstrument { ticker: string; name: string; currency: string; isin: string }

export interface T212SyncAccount {
  id: string;
  environment: 'demo' | 'live';
  broker_account_id: string;
  currency: string;
  schedule_enabled: boolean;
  next_run_at: string | null;
  backfill_cursor: string;
  backfill_complete: boolean;
  synced_through: string | null;
  last_success_at: string | null;
}

export interface T212SyncRun {
  id: string;
  account_id: string;
  trigger: 'scheduled' | 'sync_now' | 'full_history';
  status: 'queued' | 'running' | 'waiting' | 'needs_review' | 'succeeded' | 'failed' | 'cancelled';
  phase: 'request_report' | 'poll_report' | 'download_report' | 'import_report' | 'history_orders' | 'history_dividends' | 'history_transactions';
  range_from: string;
  range_to: string;
  cursor: string;
  report_from: string | null;
  report_to: string | null;
  report_id: string | null;
  next_attempt_at: string;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  error_code: string | null;
  error_message: string | null;
  inserted_count: number;
  matched_count: number;
  review_count: number;
  skipped_count: number;
}

export interface T212ImportBatchSummary {
  id: string;
  account_id: string | null;
  sync_run_id: string | null;
  statement_job_id: string | null;
  source: 'trading212_api' | 'statement_csv';
  status: 'staged' | 'needs_review' | 'applied' | 'failed';
  report_from: string | null;
  report_to: string | null;
  created_at: string;
  applied_at: string | null;
  inserted_count: number;
  matched_count: number;
  review_count: number;
  skipped_count: number;
  skipped_actions: Record<string, number>;
}

export interface T212SyncStatus {
  available: boolean;
  unavailable_reason: string | null;
  current_account_id: string | null;
  accounts: T212SyncAccount[];
  runs: T212SyncRun[];
  review_batches: T212ImportBatchSummary[];
}

export interface T212ReviewCandidate {
  kind: string;
  ledger_id: string;
  event_id: string | null;
  account_id: string | null;
  import_source: string | null;
  imported_at: string | null;
  native: Record<string, unknown>;
  recorded: Record<string, unknown>;
  values_differ: boolean;
}

export interface T212ReviewItem {
  batch_id: string;
  row_number: number;
  version: number;
  source: string;
  account_id: string | null;
  raw_id: string | null;
  raw_action: string;
  kind: string;
  native: Record<string, unknown>;
  normalized: Record<string, unknown>;
  reason: string;
  candidates: T212ReviewCandidate[];
}

export interface T212ReviewPage {
  items: T212ReviewItem[];
  next_cursor: number | null;
}

interface T212ReviewLocation {
  batch_id: string;
  row_number: number;
  version: number;
  account_id: string | null;
}
export type T212ReviewDecision = T212ReviewLocation & (
  | { action: 'import_new' }
  | { action: 'match_existing'; kind: string; ledger_id: string; accept_recorded_values: boolean }
);
