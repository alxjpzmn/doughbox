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
