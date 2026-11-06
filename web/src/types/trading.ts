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

export interface Proposal {
  id: string;
  client_request_id: string;
  title: string;
  note: string;
  environment: TradingEnvironment;
  status: 'pending' | 'executing' | 'completed' | 'discarded';
  created_at: string;
  trades: TradeResult[];
}

export interface TradingState {
  configured: boolean;
  environment: TradingEnvironment | null;
  account: Record<string, unknown> | null;
  agent_token_set: boolean;
  mcp_path: string;
  setup_error: string | null;
  proposals: Proposal[];
}
