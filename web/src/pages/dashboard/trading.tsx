import { useLayoutEffect, useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import { ArrowClockwiseIcon, CheckCircleIcon, CircleDashedIcon, ClockIcon, SpinnerGapIcon, WarningCircleIcon, XCircleIcon } from '@phosphor-icons/react';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { focusedProposalId } from '@/hooks/useAuth';
import type { Proposal, TradeResult, TradingEnvironment, TradingState } from '@/types/trading';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

const tradingUrl = `${BASE_URL}/trading`;
const proposalStatuses: Record<Proposal['status'], string> = {
  pending: 'Awaiting approval',
  executing: 'Submitting orders',
  completed: 'Processed',
  discarded: 'Discarded',
};
const tradeStatuses: Record<TradeResult['status'], { label: string; Icon: typeof CheckCircleIcon; color: string; spin?: boolean }> = {
  pending: { label: 'Not submitted', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' },
  submitting: { label: 'Submitting', Icon: SpinnerGapIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100', spin: true },
  submitted: { label: 'Submitted', Icon: ClockIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' },
  open: { label: 'Open', Icon: ClockIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' },
  partial: { label: 'Partially filled', Icon: WarningCircleIcon, color: 'border-amber-200 bg-amber-100 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100' },
  filled: { label: 'Filled', Icon: CheckCircleIcon, color: 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100' },
  cancelled: { label: 'Cancelled', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' },
  rejected: { label: 'Rejected', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' },
  failed: { label: 'Failed', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' },
  unknown: { label: 'Outcome unknown', Icon: WarningCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' },
  discarded: { label: 'Discarded', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' },
};

function errorMessage(failure: unknown): string {
  return failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
    ? failure.message : 'Trading is unavailable. Please refresh before taking further action.';
}

function StatusIndicator({ state, label }: { state: 'running' | 'succeeded' | 'failed'; label: string }) {
  const presentation = {
    running: { Icon: SpinnerGapIcon, spin: true },
    succeeded: { Icon: CheckCircleIcon, spin: false },
    failed: { Icon: XCircleIcon, spin: false },
  }[state];
  const { Icon, spin } = presentation;
  return <span role="status" className="inline-flex items-center gap-1 text-xs font-medium" aria-label={label}>
    <Icon aria-hidden="true" className={`size-3 ${spin ? 'motion-safe:animate-spin' : ''}`} />
    {label}
  </span>;
}

function RefreshControl({ label, busy, disabled, feedback, onClick }: {
  label: string;
  busy: boolean;
  disabled: boolean;
  feedback: { error: boolean; message: string; refresh?: boolean } | null;
  onClick: () => void;
}) {
  const refreshFeedback = feedback?.refresh ? feedback : null;
  return <div className="space-y-2">
    <div className="flex flex-wrap items-center gap-3">
      <Button type="button" variant="outline" size="sm" disabled={disabled || busy} onClick={onClick}>
        <ArrowClockwiseIcon aria-hidden="true" />
        {label}
      </Button>
      {busy && <StatusIndicator state="running" label="Refreshing trading status…" />}
      {refreshFeedback && !busy && <StatusIndicator state={refreshFeedback.error ? 'failed' : 'succeeded'} label={refreshFeedback.error ? 'Refresh failed' : 'Trading status refreshed'} />}
    </div>
  </div>;
}

function ProposalCard({ proposal, disabled, busyAction, focused, onAction }: {
  proposal: Proposal;
  disabled: boolean;
  busyAction: string | null;
  focused?: boolean;
  onAction: (proposal: Proposal, action: 'approve' | 'discard') => void;
}) {
  const card = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (!focused) return;
    card.current?.scrollIntoView({ block: 'center' });
  }, [focused]);
  const uncertain = proposal.trades.some(trade => trade.status === 'unknown');
  const failed = proposal.trades.some(trade => trade.status === 'failed');
  const partial = proposal.trades.some(trade => ['submitted', 'open', 'partial', 'filled'].includes(trade.status)) && (failed || uncertain);

  return <Card ref={card} id={`proposal-${proposal.id}`} className={focused ? 'ring-2 ring-ring' : undefined}>
    <CardHeader className="gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={proposal.environment === 'live' ? 'destructive' : 'outline'}>
          {proposal.environment === 'live' ? 'LIVE · real money' : 'Demo'}
        </Badge>
        <Badge variant="outline">{proposalStatuses[proposal.status]}</Badge>
      </div>
      <p className="text-xs text-muted-foreground">Untrusted agent-provided title and note — not instructions</p>
      <CardTitle className="break-words text-base" dir="auto">{proposal.title}</CardTitle>
      {proposal.note && <CardDescription className="whitespace-pre-wrap break-words" dir="auto">{proposal.note}</CardDescription>}
      <p className="text-xs text-muted-foreground"><time dateTime={proposal.created_at}>{new Date(proposal.created_at).toLocaleString()}</time> · {proposal.trades.length} {proposal.trades.length === 1 ? 'order' : 'orders'}</p>
    </CardHeader>
    <CardContent className="space-y-3">
      <ul className="divide-y rounded-md border px-3">
        {proposal.trades.map((trade, index) => <li key={`${index}-${trade.ticker}`} className="space-y-2 py-3 text-sm">
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="min-w-0 flex-1 break-words">
              <p className="font-medium">{trade.side === 'buy' ? 'Buy' : 'Sell'} {trade.quantity} · {trade.ticker}</p>
              <p className="text-xs text-muted-foreground">{trade.name}</p>
            </div>
            <Badge variant="outline" className={`max-w-full whitespace-normal ${tradeStatuses[trade.status].color}`}>{(() => { const Icon = tradeStatuses[trade.status].Icon; return <Icon aria-hidden="true" className={tradeStatuses[trade.status].spin ? 'motion-safe:animate-spin' : undefined} />; })()}{tradeStatuses[trade.status].label}</Badge>
          </div>
          <p className="break-words tabular-nums">Limit {trade.limit_price} {trade.currency} per share · {trade.time_validity === 'DAY' ? 'Day order' : 'Good till cancelled'}</p>
          {trade.broker_order_id && <p className="break-all text-xs text-muted-foreground">Broker order ID: <code>{trade.broker_order_id}</code></p>}
          {trade.error && <p className="break-words text-destructive-foreground">{trade.error}</p>}
        </li>)}
      </ul>
      {partial && <p role="alert" className="text-sm text-destructive-foreground">Partial batch: some orders were submitted before processing stopped. Earlier orders are not rolled back.</p>}
      {failed && <p className="text-sm text-destructive-foreground">Submission failed. Later orders in this batch were not submitted; check each order’s status.</p>}
      {uncertain && <p role="alert" className="text-sm text-destructive-foreground">An order may have reached the broker. Check the broker directly before any further trading. Do not retry or submit a replacement blindly.</p>}
      {proposal.status === 'pending' && <div className="flex flex-wrap gap-2">
        <Button type="button" disabled={disabled} variant={proposal.environment === 'live' ? 'destructive' : 'default'}
          onClick={() => onAction(proposal, 'approve')}>
          {busyAction === `approve:${proposal.id}` ? 'Submitting…' : `Approve ${proposal.trades.length > 1 ? 'entire batch' : 'order'}`}
        </Button>
        <Button type="button" variant="outline" disabled={disabled} onClick={() => onAction(proposal, 'discard')}>
          {busyAction === `discard:${proposal.id}` ? 'Discarding…' : 'Discard'}
        </Button>
      </div>}
    </CardContent>
  </Card>;
}

export default function Trading() {
  const submitting = useRef(false);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const { data, error, isValidating, mutate } = useSwr<TradingState>(tradingUrl, fetcher, {
    refreshInterval: busyAction ? 0 : 10_000,
    revalidateOnFocus: !busyAction,
    revalidateOnReconnect: !busyAction,
  });
  const [environment, setEnvironment] = useState<TradingEnvironment>('demo');
  const [apiKey, setApiKey] = useState('');
  const [apiSecret, setApiSecret] = useState('');
  const [editingConnection, setEditingConnection] = useState(false);
  const [copied, setCopied] = useState<'endpoint' | 'token' | null>(null);
  // The one-time token lives only in this route's component state, never in the SWR cache or browser storage.
  const [agentToken, setAgentToken] = useState<string | null>(null);
  const [refreshRequired, setRefreshRequired] = useState(false);
  const [feedback, setFeedback] = useState<{ error: boolean; message: string; refresh?: boolean } | null>(null);
  const busy = busyAction !== null;
  const stale = !!error || refreshRequired;
  const executing = data?.proposals.some(proposal => proposal.status === 'executing') ?? false;
  const disabled = busy || stale || !data;
  const setupBlocked = !!data?.setup_error;
  const ready = !!data?.configured && !!data?.agent_token_set;
  const endpoint = data?.configured ? new URL(data.mcp_path, window.location.origin).href : null;
  const pending = data?.proposals.filter(proposal => proposal.status === 'pending') ?? [];
  const recent = data?.proposals.filter(proposal => proposal.status !== 'pending') ?? [];
  const focusedId = focusedProposalId();
  const focusedFound = !!focusedId && (pending.some(proposal => proposal.id === focusedId) || recent.some(proposal => proposal.id === focusedId));

  const refresh = async () => {
    if (submitting.current) return;
    submitting.current = true;
    setBusyAction('refresh');
    try {
      await mutate(async () => await fetcher(tradingUrl) as TradingState, { revalidate: false });
      setRefreshRequired(false);
      setFeedback({ error: false, message: 'Trading status refreshed.', refresh: true });
    } catch (failure) {
      setRefreshRequired(true);
      setFeedback({ error: true, message: errorMessage(failure), refresh: true });
    } finally {
      submitting.current = false;
      setBusyAction(null);
    }
  };

  const runMutation = async (action: string, operation: () => Promise<{ message: string; state?: TradingState }>) => {
    if (submitting.current || disabled) return;
    submitting.current = true;
    setBusyAction(action);
    setFeedback(null);
    try {
      let message = '';
      let mutationSucceeded = false;
      let failure: unknown = null;
      // A rejected connection must not become the SWR error, or the form locks behind a refresh failure.
      await mutate(async (current) => {
        try {
          const result = await operation();
          mutationSucceeded = true;
          message = result.message;
          return result.state ?? await fetcher(tradingUrl) as TradingState;
        } catch (error) {
          failure = error;
          return current;
        }
      }, { revalidate: false });
      if (failure) {
        const uncertainOrder = action.startsWith('approve:');
        if (uncertainOrder || mutationSucceeded) setRefreshRequired(true);
        setFeedback({ error: true, message: uncertainOrder
          ? `${errorMessage(failure)} The request may have taken effect. Refresh status before further actions; do not retry uncertain orders. Check the broker directly.`
          : mutationSucceeded
            ? `${errorMessage(failure)} The change succeeded, but trading status could not be refreshed. Refresh status before further actions.`
            : errorMessage(failure) });
        return;
      }
      setRefreshRequired(false);
      setFeedback({ error: false, message });
    } finally {
      submitting.current = false;
      setBusyAction(null);
    }
  };

  const saveConnection = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (disabled || setupBlocked || executing || !apiKey.trim() || !apiSecret.trim()) return;
    if ((environment === 'live' || data?.configured) && !window.confirm(
      `${environment === 'live' ? 'Connect to LIVE trading with real money at risk? ' : 'Replace the broker connection? '}All pending proposals will be discarded. Orders already submitted to the broker are not cancelled.`,
    )) return;
    await runMutation('connection', async () => {
      const response = await sendMutateRequest(`${tradingUrl}/connection`, {
        api_key: apiKey, api_secret: apiSecret, environment,
      }, { method: 'PUT' });
      const state: TradingState = await response.json();
      setAgentToken(null);
      setApiKey('');
      setApiSecret('');
      setEditingConnection(false);
      return { message: 'Broker connection saved.', state };
    });
  };

  const disconnect = async () => {
    if (disabled || executing || !data?.configured || !window.confirm('Disconnect trading? Pending proposals will be discarded and agent access disabled. Orders already submitted to the broker are not cancelled.')) return;
    await runMutation('disconnect', async () => {
      const response = await sendMutateRequest(`${tradingUrl}/connection`, undefined, { method: 'DELETE' });
      const state: TradingState = await response.json();
      setAgentToken(null);
      setApiKey('');
      setApiSecret('');
      return { message: 'Trading disconnected. Check the broker for any open orders.', state };
    });
  };

  const createToken = async () => {
    if (disabled || setupBlocked || !data?.configured) return;
    if (data.agent_token_set && !window.confirm('Rotate the agent token? The previous token will stop working immediately.')) return;
    setCopied(current => current === 'token' ? null : current);
    setAgentToken(null);
    await runMutation('token', async () => {
      const response = await sendMutateRequest(`${tradingUrl}/agent-token`);
      const result: { token: string } = await response.json();
      setAgentToken(result.token);
      return { message: 'Agent token created. Copy it now; it will not be shown again after you leave this page.' };
    });
  };

  const proposalAction = async (proposal: Proposal, action: 'approve' | 'discard') => {
    if (disabled || !data?.configured || setupBlocked || (action === 'approve' && executing)) return;
    await runMutation(`${action}:${proposal.id}`, async () => {
      const response = await sendMutateRequest(`${tradingUrl}/proposals/${encodeURIComponent(proposal.id)}/${action}`);
      const result: Proposal = await response.json();
      return { message: action === 'discard' ? 'Proposal discarded.'
        : result.trades.some(trade => trade.status === 'failed' || trade.status === 'unknown')
          ? 'Processing stopped with failed or uncertain orders. Review the results and check the broker; do not retry blindly.'
          : 'Proposal processed. Review each order below; submitted does not mean filled.' };
    });
  };

  const copy = async (value: string, kind: 'endpoint' | 'token') => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(kind);
      window.setTimeout(() => setCopied(current => current === kind ? null : current), 2000);
    } catch {
      setFeedback({ error: true, message: `Could not copy the ${kind}. Select and copy the displayed value manually.` });
    }
  };

  return <div className="space-y-4">
    <div>
      <h1 className="text-xl font-semibold">Trading</h1>
      <p className="mt-1 text-sm text-muted-foreground">{setupBlocked
        ? 'Before connecting Trading 212, configure a login password and credential-encryption key on the server. Once setup is complete, you can connect your broker, give an external agent the MCP endpoint, and review its proposed trades here.'
        : 'External agents can only propose trades. You review and approve each order or entire batch here; an agent token cannot approve or execute trades.'}</p>
    </div>
    {!data && !error && <p role="status" className="text-sm text-muted-foreground">Loading trading…</p>}
    {(error || refreshRequired) && <div role="alert" className="space-y-2 rounded-lg border border-red-800/40 bg-red-50 p-3 text-sm text-red-950 dark:border-red-200/40 dark:bg-red-950 dark:text-red-50">
      <p className="break-words font-medium">{data ? 'Trading status could not be refreshed. Last loaded results are shown; controls are paused until refresh succeeds.' : 'Trading status could not be loaded.'} {error && errorMessage(error)}</p>
      {!setupBlocked && <RefreshControl label="Retry / refresh status" busy={busyAction === 'refresh'} disabled={busy && busyAction !== 'refresh'} feedback={feedback} onClick={refresh} />}
    </div>}
    {data?.setup_error && <div role="alert" className="space-y-2 rounded-lg border border-red-800/40 bg-red-50 p-3 text-sm text-red-950 dark:border-red-200/40 dark:bg-red-950 dark:text-red-50">
      <p className="font-semibold">Server setup required</p>
      <p className="break-words font-medium">{data.setup_error}</p>
      <p>Ask the server administrator to configure PASSWORD and TRADING_MASTER_KEY (a hex-encoded 32-byte encryption key). Broker credentials are encrypted on the server. Refresh after the server configuration is fixed.</p>
      <RefreshControl label="Refresh setup status" busy={busyAction === 'refresh'} disabled={busy && busyAction !== 'refresh'} feedback={feedback} onClick={refresh} />
    </div>}
    {feedback && !feedback.refresh && <p role={feedback.error ? 'alert' : 'status'} className={`break-words text-sm ${feedback.error ? 'text-red-700 dark:text-red-300' : 'text-muted-foreground'}`}>{feedback.message}</p>}
    {data && !setupBlocked && <Tabs defaultValue={focusedId || ready || pending.length > 0 || recent.length > 0 ? 'orders' : 'setup'} className="max-md:pb-28">
      <TabsList dock className="mb-6 h-auto sm:w-fit" aria-label="Trading sections">
        <TabsTrigger value="orders" className="min-h-11">Orders</TabsTrigger>
        <TabsTrigger value="setup" className="min-h-11">Setup</TabsTrigger>
      </TabsList>
      <TabsContent value="setup" className="space-y-4">
      <Card>
        <CardHeader>
          <CardTitle>Trading 212 connection</CardTitle>
          <CardDescription>Only Trading 212 is supported for now. Generate an API key ID and Secret Key using the <a href="https://helpcentre.trading212.com/hc/en-us/articles/14584770928157-Trading-212-API-key" target="_blank" rel="noopener noreferrer" className="underline">API key setup</a>, then see the <a href="https://docs.trading212.com/api" target="_blank" rel="noopener noreferrer" className="underline">API docs</a>. Demo is the default; live orders put real money at risk.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {executing && <p role="status" className="text-sm text-muted-foreground">Orders are being submitted. Connection changes and further approvals are disabled until processing finishes.</p>}
          {data.configured && !editingConnection ? <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
              <Badge variant="outline" className={data.environment === 'live'
                ? 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100'
                : 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100'}>
                {data.environment === 'live' ? <WarningCircleIcon aria-hidden="true" /> : <CheckCircleIcon aria-hidden="true" />}
                {data.environment === 'live' ? 'Connected · LIVE · real money' : 'Connected · Demo'}
              </Badge>
              {typeof data.account?.id === 'string' && <span className="break-all text-muted-foreground">Account {data.account.id}{typeof data.account.currency === 'string' ? ` · ${data.account.currency}` : ''}</span>}
            </div>
            <div className="flex flex-wrap gap-2">
              <Button type="button" variant="outline" disabled={disabled || executing} onClick={() => {
                setEnvironment(data.environment === 'live' ? 'live' : 'demo');
                setApiKey('');
                setApiSecret('');
                setEditingConnection(true);
              }}>Edit</Button>
              <Button type="button" variant="outline" disabled={disabled || executing} onClick={disconnect}>{busyAction === 'disconnect' ? 'Disconnecting…' : 'Disconnect'}</Button>
            </div>
          </div> : <form aria-label="Trading 212 connection" onSubmit={saveConnection} className="space-y-3" autoComplete="off">
            <div className="space-y-2">
              <Label htmlFor="trading-environment">Environment for this connection</Label>
              <Select value={environment} onValueChange={value => setEnvironment(value as TradingEnvironment)} disabled={disabled || setupBlocked || executing}>
                <SelectTrigger id="trading-environment"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="demo">Demo — practice trading</SelectItem>
                  <SelectItem value="live">Live — real money at risk</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {environment === 'live' && <p role="alert" className="text-sm font-medium text-destructive-foreground">Live trading uses real money. Review every side, quantity, currency and limit before approving.</p>}
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="trading-api-key">API key ID</Label>
                <Input id="trading-api-key" type="password" autoComplete="new-password" spellCheck={false} required value={apiKey} disabled={disabled || setupBlocked || executing} onChange={event => setApiKey(event.target.value)} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="trading-api-secret">Secret Key</Label>
                <Input id="trading-api-secret" type="password" autoComplete="new-password" spellCheck={false} required value={apiSecret} disabled={disabled || setupBlocked || executing} onChange={event => setApiSecret(event.target.value)} />
              </div>
            </div>
            <p className="text-xs text-muted-foreground">Credentials are never stored in browser storage or returned here. Saving a connection discards all pending proposals; it does not cancel existing broker orders.</p>
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={disabled || setupBlocked || executing || !apiKey.trim() || !apiSecret.trim()}>{busyAction === 'connection' ? 'Saving…' : data.configured ? 'Replace connection' : 'Connect'}</Button>
              {data.configured && <Button type="button" variant="outline" disabled={busy} onClick={() => {
                setApiKey('');
                setApiSecret('');
                setEditingConnection(false);
              }}>Cancel</Button>}
            </div>
          </form>}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Agent access</CardTitle>
          <CardDescription>Connect an external agent using the MCP endpoint and a separate Bearer token. Keep this token private; it grants proposal and instrument-search access, not approval rights.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {!data.configured ? <p className="text-sm text-muted-foreground">Connect a broker first to enable the MCP endpoint and agent token controls.</p> : <>
            <Label htmlFor="trading-mcp-endpoint">MCP endpoint</Label>
            <div className="flex flex-col gap-2 sm:flex-row">
              <Input id="trading-mcp-endpoint" className="min-w-0" readOnly value={endpoint ?? ''} disabled={stale || setupBlocked} />
              <Button type="button" variant="outline" disabled={disabled || setupBlocked || !endpoint} onClick={() => endpoint && copy(endpoint, 'endpoint')}>{copied === 'endpoint' ? <><CheckCircleIcon aria-hidden="true" />Copied</> : 'Copy endpoint'}</Button>
            </div>
            <p className="text-xs text-muted-foreground">In your agent’s remote HTTP MCP settings, use this URL and an Authorization header of <code>Bearer &lt;agent token&gt;</code>.</p>
            <p className="text-xs text-muted-foreground">For a public hostname, the server administrator must include it in <code>TRADING_MCP_HOSTS</code>. Use HTTPS outside localhost. Proposal responses include a link back to this page. Set <code>DOUGHBOX_PUBLIC_URL</code> when the browser origin differs from the MCP Host.</p>
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" variant="outline" disabled={disabled || setupBlocked} onClick={createToken}>{busyAction === 'token' ? 'Creating…' : data.agent_token_set ? 'Rotate agent token' : 'Create agent token'}</Button>
              <span className="text-xs text-muted-foreground">{data.agent_token_set ? 'A token is configured. Rotation revokes the previous token.' : 'No agent token configured.'}</span>
            </div>
            {agentToken && !stale && !setupBlocked && <div className="space-y-2 rounded-md border p-3">
              <Label htmlFor="trading-agent-token">New agent token — shown only this time</Label>
              <Input id="trading-agent-token" readOnly value={agentToken} autoComplete="off" spellCheck={false} />
              <p className="text-xs text-muted-foreground">Copy it now into your agent’s secret configuration. Leaving this page clears this display. Copying places the secret on your system clipboard.</p>
              <div className="flex flex-wrap gap-2">
                <Button type="button" variant="outline" disabled={busy} onClick={() => copy(agentToken, 'token')}>{copied === 'token' ? <><CheckCircleIcon aria-hidden="true" />Copied</> : 'Copy token'}</Button>
                <Button type="button" variant="ghost" onClick={() => setAgentToken(null)}>Hide token</Button>
              </div>
            </div>}
          </>}
        </CardContent>
      </Card>
      </TabsContent>
      <TabsContent value="orders" className="space-y-4">
      <section aria-labelledby="trading-pending-title" className="space-y-3">
        <h2 id="trading-pending-title" className="text-base font-semibold">Pending proposals ({pending.length})</h2>
        {focusedId && <p role="status" className="text-sm text-muted-foreground">Opened from an approval link. Review the matching proposal below. Nothing is sent to the broker until you approve it here.</p>}
        {focusedId && !focusedFound && <p role="status" className="text-sm text-muted-foreground">This link does not match a pending or recent proposal. It may be older than the latest 50 processed proposals.</p>}
        {!error && !refreshRequired && <RefreshControl label="Refresh" busy={busyAction === 'refresh'} disabled={(busy && busyAction !== 'refresh') || isValidating} feedback={feedback} onClick={refresh} />}
        {pending.length === 0 && <p className="rounded-md border p-4 text-sm text-muted-foreground">No proposals awaiting approval. A connected agent can submit a single order or a batch for your review.</p>}
        {pending.map(proposal => <ProposalCard key={proposal.id} proposal={proposal} focused={proposal.id === focusedId} disabled={disabled || setupBlocked || !data.configured || executing || proposal.environment !== data.environment} busyAction={busyAction} onAction={proposalAction} />)}
      </section>
      <details className="rounded-lg border p-4" open={recent.some(proposal => proposal.id === focusedId) ? true : undefined}>
        <summary className="cursor-pointer text-base font-semibold">Recent history ({recent.length})</summary>
        <div className="space-y-3 pt-3">
          {recent.length === 0
            ? <p className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">No processed or discarded proposals yet. Approved and discarded proposals will appear here.</p>
            : <>
              <p className="text-xs text-muted-foreground">Open, filled, partial, cancelled, and rejected states are checked against <a href="https://docs.trading212.com/api/orders" target="_blank" rel="noopener noreferrer" className="underline">Trading 212’s orders API</a> and recent history. There is no automatic retry.</p>
              {recent.map(proposal => <ProposalCard key={proposal.id} proposal={proposal} focused={proposal.id === focusedId} disabled busyAction={busyAction} onAction={proposalAction} />)}
            </>}
        </div>
      </details>
      </TabsContent>
    </Tabs>}
  </div>;
}
