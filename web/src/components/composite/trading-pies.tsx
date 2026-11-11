import { useLayoutEffect, useRef, useState, type FormEvent } from 'react';
import { ArrowRightIcon, ChartPieSliceIcon } from '@phosphor-icons/react';
import { BASE_URL, fetcher, getPrivateRequestEpoch, isCurrentPrivateRequestEpoch, type HttpError } from '@/lib/http';
import type { DividendCashAction, PieDetail, PieList, PieOperationInput, PieSettings, PieSettingsChanges, Proposal, SubmitPieInput, TradingInstrument } from '@/types/trading';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

const url = `${BASE_URL}/trading`;
type PieProposal = Extract<Proposal, { kind: 'pie' }>;
type Draft = {
  action: PieOperationInput['action'];
  key: string;
  title: string;
  note: string;
  source: PieDetail | null;
  name: string;
  icon: string;
  goal: string;
  endDate: string;
  dividend: string;
  allocations: { ticker: string; weight: string }[];
};

// Decimal strings stay strings (and integer arithmetic) throughout presentation and validation.
function decimalParts(value: string): { coefficient: bigint; scale: number } | null {
  const match = /^([+-]?)(\d+)(?:\.(\d+))?(?:[eE]([+-]?\d+))?$/.exec(value);
  if (!match) return null;
  const coefficient = BigInt(`${match[1] === '-' ? '-' : ''}${match[2]}${match[3] ?? ''}`);
  const exponent = Number(match[4] ?? 0);
  const scale = (match[3]?.length ?? 0) - exponent;
  if (!Number.isSafeInteger(exponent) || !Number.isSafeInteger(scale)) return null;
  if (coefficient === 0n) return { coefficient, scale: 0 };
  return scale < 0 ? { coefficient: coefficient * 10n ** BigInt(-scale), scale: 0 } : { coefficient, scale };
}
function decimalText(coefficient: bigint, scale: number): string {
  const negative = coefficient < 0n;
  const digits = (negative ? -coefficient : coefficient).toString().padStart(scale + 1, '0');
  const text = scale ? `${digits.slice(0, -scale)}.${digits.slice(-scale)}`.replace(/\.?0+$/, '') : digits;
  return `${negative ? '-' : ''}${text || '0'}`;
}
export function piePercentage(value: string | null | undefined): string {
  if (value == null) return 'Unknown';
  if (value === '') return '—';
  const parts = decimalParts(value);
  return parts ? `${decimalText(parts.coefficient * 100n, parts.scale)}%` : `${value} (fraction)`;
}

function summaryNumber(value: string | null | undefined, minimumFractionDigits = 2): string {
  if (value == null) return 'Unknown';
  if (!decimalParts(value)) return value;
  // ECMA-402 formats decimal strings exactly; TypeScript's Intl signature only
  // lists number/bigint. Never convert broker amounts through a JS Number.
  const format = new Intl.NumberFormat(undefined, { minimumFractionDigits, maximumFractionDigits: 2 }).format as (value: string) => string;
  return format(value);
}

function summaryPercentage(value: string | null | undefined): string {
  const parts = value == null ? null : decimalParts(value);
  return parts ? `${summaryNumber(decimalText(parts.coefficient * 100n, parts.scale), 0)}%` : piePercentage(value);
}
function sameDecimal(left: string | null | undefined, right: string | null | undefined): boolean {
  if (left == null || right == null) return left === right;
  const a = decimalParts(left), b = decimalParts(right);
  return a && b ? a.coefficient * 10n ** BigInt(b.scale) === b.coefficient * 10n ** BigInt(a.scale) : left === right;
}
function sameTargets(left: PieSettings['instrument_shares'], right: PieSettings['instrument_shares']): boolean {
  if (!left || !right) return left === right;
  const keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every(ticker => ticker in right && sameDecimal(left[ticker], right[ticker]));
}
function failureMessage(error: unknown): string {
  const failure = error as Partial<HttpError>;
  if (failure?.error === 'TradingRateLimited' || failure?.status === 429) {
    const details = failure.details as { retry_after_seconds?: number } | undefined;
    return `Trading 212 request limit reached. Wait ${details?.retry_after_seconds ?? failure.retryAfter ?? 'the indicated number of'} seconds, then refresh explicitly. Nothing will be retried automatically.`;
  }
  return failure?.message ?? 'The request could not be completed. Refresh explicitly before using stale broker data.';
}
function writable(detail: PieDetail | null): boolean {
  const settings = detail?.settings;
  return !!settings?.name && !!settings.dividend_cash_action && !!settings.instrument_shares
    && Object.keys(settings.instrument_shares).length > 0
    && Object.values(settings.instrument_shares).every(weight => weight !== null);
}
function emptyHoldings(detail: PieDetail): boolean {
  return detail.instruments !== null && detail.instruments.every(instrument => instrument.owned_quantity !== null && sameDecimal(instrument.owned_quantity, '0'));
}
function PieMetric({ label, value, title, labelTitle, tone, prominent, hint }: { label: string; value: string; title?: string; labelTitle?: string; tone?: string; prominent?: boolean; hint?: string }) {
  return <div className="min-w-0">
    <dt title={labelTitle} className="text-xs text-muted-foreground">{label}</dt>
    <dd title={title} className={`mt-1 break-words tabular-nums ${prominent ? 'text-xl font-semibold tracking-tight' : 'font-medium'} ${tone ?? ''}`}>{value}</dd>
    {hint && <dd className={`mt-0.5 break-words text-xs tabular-nums ${tone ?? ''}`}>{hint}</dd>}
  </div>;
}
function Settings({ settings, label, currency, changed }: { settings: PieSettings | null; label: string; currency?: string; changed?: readonly string[] }) {
  const shares = settings?.instrument_shares;
  return <article className="min-w-0 overflow-hidden rounded-xl border bg-card">
    <div className="flex items-center gap-3 p-4">
      <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"><ChartPieSliceIcon aria-hidden="true" className="size-5" /></span>
      <span className="min-w-0 flex-1">
        <span className="block break-words font-semibold">{settings?.name ?? 'Name not provided'}</span>
        <span className="block text-xs text-muted-foreground">{label}</span>
      </span>
    </div>
    {settings ? <div className="space-y-4 p-4 pt-0">
      <dl className="grid grid-cols-2 gap-4">
        <PieMetric prominent label={`Goal${currency ? ` · ${currency}` : ''}`} value={settings.goal == null ? 'Not provided' : summaryNumber(settings.goal)} title={settings.goal ?? undefined} />
        <PieMetric prominent label="Dividends" value={settings.dividend_cash_action ?? 'Not provided'} />
      </dl>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-3 border-t pt-3 text-xs">
        <PieMetric label="End date" value={settings.end_date ?? 'Not provided'} />
        <PieMetric label="Icon" value={settings.icon ?? 'Not provided'} />
      </dl>
      <div className="space-y-2 rounded-lg bg-muted/40 p-3">
        <p className="text-xs font-medium">Configured target weights</p>
        {settings.instrument_shares_source === 'expected_share' && <p className="text-xs text-muted-foreground">Reconstructed from each instrument's expected share. Trading 212 omitted instrumentShares. These are not owned quantities.</p>}
        {shares ? <ul className="grid gap-2 sm:grid-cols-2">{Object.entries(shares).sort(([a], [b]) => a.localeCompare(b)).map(([ticker, weight]) => <li key={ticker} className={`min-w-0 ${changed?.includes(ticker) ? 'rounded-md bg-background p-2 ring-1 ring-border' : ''}`}>
          <p className="break-all text-xs text-muted-foreground"><code>{ticker}</code></p>
          <p className="mt-1 break-words text-sm font-semibold tabular-nums">{weight == null ? 'Unknown' : summaryPercentage(weight)}</p>
          <p className="break-all text-xs text-muted-foreground">{weight ?? 'Unknown'} weight</p>
        </li>)}</ul> : <p className="text-xs text-muted-foreground">Target allocation evidence unavailable.</p>}
      </div>
    </div> : <p className="px-4 pb-4 text-sm text-muted-foreground">Settings evidence unavailable.</p>}
  </article>;
}
function Holdings({ detail, currency }: { detail: PieDetail; currency?: string }) {
  return <section className="min-w-0 space-y-3 overflow-hidden rounded-xl border bg-card p-4">
    <h4 className="font-medium">Actual broker holdings and weights</h4>
    {detail.instruments === null ? <p className="text-sm text-muted-foreground">Holdings evidence unavailable; this is not a zero balance.</p>
      : detail.instruments.length === 0 ? <p className="text-sm text-muted-foreground">No instruments reported. Free Pie cash is not independently established by this response.</p>
      : <ul className="grid gap-3 sm:grid-cols-2">{detail.instruments.map((instrument, index) => <li key={`${instrument.ticker}-${index}`} className="min-w-0 rounded-lg bg-muted/40 p-3 text-sm">
        <p className="break-all font-semibold">{instrument.ticker ?? 'Unknown instrument'}</p>
        <dl className="mt-3 grid grid-cols-2 gap-3">
          <PieMetric prominent label="Owned quantity" value={instrument.owned_quantity ?? 'Unknown'} />
          <PieMetric prominent label="Current weight" value={piePercentage(instrument.current_share)} />
        </dl>
        <p className="mt-2 text-xs text-muted-foreground">Broker expected weight: {piePercentage(instrument.expected_share)}</p>
        {instrument.result && <dl className="mt-3 grid grid-cols-2 gap-x-3 gap-y-2 border-t pt-3 text-xs">
          <PieMetric label={`Current value${currency ? ` · ${currency}` : ''}`} value={summaryNumber(instrument.result.price_avg_value)} title={instrument.result.price_avg_value ?? undefined} />
          <PieMetric label="Invested value" value={summaryNumber(instrument.result.price_avg_invested_value)} title={instrument.result.price_avg_invested_value ?? undefined} />
          <PieMetric label="Return" value={summaryNumber(instrument.result.price_avg_result)} title={instrument.result.price_avg_result ?? undefined} />
          <PieMetric label="Return %" value={summaryPercentage(instrument.result.price_avg_result_coef)} title={instrument.result.price_avg_result_coef ?? undefined} />
        </dl>}
        {instrument.issues === null ? <p className="mt-2 text-xs text-muted-foreground">Broker issues not provided.</p> : instrument.issues.map((issue, issueIndex) => <p key={issueIndex} className="mt-2 break-words text-xs text-amber-800 dark:text-amber-200">{issue.severity ?? 'Issue'}: {issue.name ?? 'Unspecified broker issue'}</p>)}
      </li>)}</ul>}
  </section>;
}

export function PieProposalCard({ proposal, disabled, readbackDisabled, busyAction, focused, onAction, onReconcile }: {
  proposal: PieProposal;
  disabled: boolean;
  readbackDisabled: boolean;
  busyAction: string | null;
  focused?: boolean;
  onAction: (proposal: Proposal, action: 'approve' | 'discard', confirmEmpty?: boolean) => void;
  onReconcile: (proposal: PieProposal) => void;
}) {
  const card = useRef<HTMLDivElement>(null);
  const [confirmEmpty, setConfirmEmpty] = useState(false);
  useLayoutEffect(() => { if (focused) card.current?.scrollIntoView({ block: 'center' }); }, [focused]);
  const pie = proposal.pie, mutation = pie.mutation;
  const currency = pie.account_currency ?? undefined;
  const labels: Record<typeof pie.status, string> = {
    pending: 'Awaiting approval', submitting: 'Submitting', submitted: 'Submitted · verification pending',
    confirmed: mutation.action === 'delete' ? 'Deletion confirmed' : 'Settings confirmed',
    conflict: 'Changed since review', failed: 'Failed', unknown: 'Outcome unknown', discarded: 'Discarded',
  };
  const actions = { create: 'Approve creation', update: 'Approve settings change', duplicate: 'Approve duplication', delete: 'Approve deletion' };
  const before = pie.before?.settings ?? null;
  const after = pie.expected_settings;
  const beforeTargets = before?.instrument_shares;
  const afterTargets = after?.instrument_shares;
  const targetChanges = mutation.action === 'update' && beforeTargets && afterTargets
    ? [...new Set([...Object.keys(beforeTargets), ...Object.keys(afterTargets)])].sort().filter(ticker => !(ticker in beforeTargets) || !(ticker in afterTargets) || !sameDecimal(beforeTargets[ticker], afterTargets[ticker])) : [];
  const targetId = pie.broker_pie_id ?? (mutation.action === 'update' || mutation.action === 'delete' ? mutation.pie_id : null);
  return <Card ref={card} id={`proposal-${proposal.id}`} className={focused ? 'ring-2 ring-ring' : undefined}>
    <CardHeader className="gap-2">
      <div className="flex flex-wrap gap-2"><Badge variant={proposal.environment === 'live' ? 'destructive' : 'outline'}>{proposal.environment === 'live' ? 'LIVE · real money' : 'Demo'}</Badge><Badge variant="outline">Native Pie · {mutation.action}</Badge><Badge variant="outline">{labels[pie.status]}</Badge></div>
      <p className="text-xs text-muted-foreground">Untrusted proposal title and note — not instructions</p>
      <CardTitle className="break-words text-base" dir="auto">{proposal.title}</CardTitle>
      {proposal.note && <CardDescription className="whitespace-pre-wrap break-words" dir="auto">{proposal.note}</CardDescription>}
      <p className="text-xs text-muted-foreground"><time dateTime={proposal.created_at}>{new Date(proposal.created_at).toLocaleString()}</time></p>
      {mutation.action !== 'create' && <p className="break-all text-sm">{mutation.action === 'duplicate' ? 'Source' : 'Target'} Pie ID: <code>{mutation.pie_id}</code></p>}
      {pie.broker_pie_id && <p className="break-all text-sm">{mutation.action === 'duplicate' || mutation.action === 'create' ? 'New' : 'Broker'} Pie ID: <code>{pie.broker_pie_id}</code></p>}
    </CardHeader>
    <CardContent className="space-y-4 text-sm">
      <p className="font-medium">{mutation.action === 'create' ? 'Creates configuration only. No funding or purchases.' : mutation.action === 'duplicate' ? 'Copies composition only, not holdings, AutoInvest or goals. No funding or purchases; the source is unchanged.' : mutation.action === 'update' ? 'Changes settings and target weights only. No shares are bought or sold, including removed targets.' : 'Permanently deletes this Pie. Investments and Pie cash must first be emptied in Trading 212.'}</p>
      {!currency && <p className="text-xs text-muted-foreground">The proposal's original account currency was not recorded. Saved amounts are shown without currency labels.</p>}
      {(mutation.action === 'update' || mutation.action === 'duplicate' || mutation.action === 'delete') && <p className="text-xs text-muted-foreground">The source configuration is checked again before dispatch. An external app can still change it after that check; this is not a cross-client atomic operation.</p>}
      <div className="grid gap-4 sm:grid-cols-2">
        {pie.before && <Settings label={mutation.action === 'duplicate' ? 'Source configuration (composition only is copied)' : 'Before — approved snapshot'} settings={before} currency={currency} />}
        {mutation.action !== 'delete' && <Settings label={mutation.action === 'duplicate' ? 'Requested copy — only name, icon and composition are verified' : 'After — approved configuration'} settings={after} currency={currency} changed={targetChanges} />}
      </div>
      {mutation.action === 'duplicate' && <p className="break-words">Requested copy name: {mutation.name} · Icon: {mutation.icon ?? 'Not supplied; broker default is not assumed'}</p>}
      {targetChanges.length > 0 && <section><h4 className="font-medium">Exact allocation changes</h4><ul className="mt-2 space-y-2">{targetChanges.map(ticker => {
        const removed = !(ticker in afterTargets!);
        const holding = pie.before?.instruments?.find(instrument => instrument.ticker === ticker);
        return <li key={ticker} className="break-words rounded-md border p-2"><code>{ticker}</code>: {!(ticker in beforeTargets!) ? 'Added' : removed ? 'Removed' : 'Weight changed'} · {beforeTargets![ticker] ?? 'Not targeted'} → {afterTargets![ticker] ?? 'Not targeted'}{removed && <p>Retained holding in snapshot: {holding?.owned_quantity ?? 'Unknown'} shares. Removing a target does not sell these shares.</p>}</li>;
      })}</ul></section>}
      {pie.before && <details><summary className="cursor-pointer font-medium">Holdings at review time (not executed trades)</summary><div className="pt-2"><Holdings detail={pie.before} currency={currency} /></div></details>}
      {pie.error && <p role="alert" className="whitespace-pre-wrap break-words text-red-700 dark:text-red-300">{pie.error}</p>}
      {pie.status === 'conflict' && <p>No broker mutation was dispatched because the saved source no longer matches. Refresh the Pie and prepare a new proposal.</p>}
      {pie.status === 'submitted' && <p>The broker acknowledged the request, but the intended configuration/absence has not yet been verified. No automatic write retry occurs.</p>}
      {pie.status === 'unknown' && <p role="alert">The request may have reached Trading 212. {targetId ? 'Observed state cannot prove this request executed. Check the broker before proposing any replacement.' : 'No returned broker ID is available. Check Trading 212 manually; this app will not search by name, attribute a copy, or resubmit the mutation.'}</p>}
      {pie.submitted_at && <p className="text-xs text-muted-foreground">Acknowledged at <time dateTime={pie.submitted_at}>{pie.submitted_at}</time></p>}
      {pie.observation && <section className="space-y-2">
        <h4 className="font-medium">Observed broker state · <time dateTime={pie.observation.observed_at}>{pie.observation.observed_at}</time></h4>
        {pie.observation.state === 'absent' ? <p>Authoritative ID lookup: Pie absent.</p>
          : pie.observation.pie ? <>
            <p className="break-all">Observed Pie ID: {pie.observation.pie.id}</p>
            <Settings label="Actual readback settings" settings={pie.observation.pie.settings} currency={currency} />
            <Holdings detail={pie.observation.pie} currency={currency} />
          </> : <p>Present; detail unavailable.</p>}
      </section>}
      {proposal.status === 'pending' && <div className="space-y-3">
        {mutation.action === 'delete' && <Label className="flex items-start gap-2"><input type="checkbox" className="mt-1 size-4 shrink-0" checked={confirmEmpty} disabled={disabled} onChange={event => setConfirmEmpty(event.target.checked)} />I have emptied this Pie&apos;s investments and Pie cash in Trading 212</Label>}
        <div className="flex flex-wrap gap-2"><Button type="button" variant={mutation.action === 'delete' || proposal.environment === 'live' ? 'destructive' : 'default'} disabled={disabled || (mutation.action === 'delete' && !confirmEmpty)} onClick={() => onAction(proposal, 'approve', mutation.action === 'delete' && confirmEmpty)}>{busyAction === `approve:${proposal.id}` ? 'Submitting…' : actions[mutation.action]}</Button><Button type="button" variant="outline" disabled={disabled} onClick={() => onAction(proposal, 'discard')}>Discard</Button></div>
      </div>}
      {(pie.status === 'submitted' || pie.status === 'unknown') && targetId && <Button type="button" variant="outline" disabled={readbackDisabled} onClick={() => onReconcile(proposal)}>{busyAction === `reconcile:${proposal.id}` ? 'Checking…' : 'Check broker state'}</Button>}
      {pie.empty_confirmed_by_owner && <p className="text-xs text-muted-foreground">The owner acknowledged empty investments and Pie cash before dispatch. Public cash evidence is incomplete; this is not an independent cash-balance verification.</p>}
    </CardContent>
  </Card>;
}

export default function TradingPies({ disabled, busyAction, currency, invalidation, onSubmit }: {
  disabled: boolean;
  busyAction: string | null;
  currency?: string;
  invalidation: number;
  onSubmit: (input: SubmitPieInput) => Promise<boolean>;
}) {
  const mounted = useRef(false);
  const reading = useRef(false);
  const requestVersion = useRef(0);
  const previousInvalidation = useRef(invalidation);
  const [list, setList] = useState<PieList | null>(null);
  const [detail, setDetail] = useState<PieDetail | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [readBusy, setReadBusy] = useState<string | null>(null);
  const [listStale, setListStale] = useState(false);
  const [detailStale, setDetailStale] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [query, setQuery] = useState('');
  const [matches, setMatches] = useState<TradingInstrument[]>([]);
  const [searched, setSearched] = useState(false);
  useLayoutEffect(() => { mounted.current = true; return () => { mounted.current = false; requestVersion.current += 1; }; }, []);
  useLayoutEffect(() => {
    if (previousInvalidation.current === invalidation) return;
    previousInvalidation.current = invalidation;
    requestVersion.current += 1;
    reading.current = false;
    setReadBusy(null);
    setListStale(true);
    setDetailStale(true);
  }, [invalidation]);
  const current = (epoch: number, version: number) => mounted.current && isCurrentPrivateRequestEpoch(epoch) && version === requestVersion.current;
  const read = async (kind: 'list' | 'detail' | 'search', id?: string) => {
    if (reading.current || disabled) return;
    reading.current = true;
    const epoch = getPrivateRequestEpoch(), version = ++requestVersion.current;
    setReadBusy(kind);
    setReadError(null);
    try {
      if (kind === 'list') {
        const result: PieList = await fetcher(`${url}/pies`);
        if (!current(epoch, version)) return;
        setList(result); setListStale(false);
      } else if (kind === 'detail' && id) {
        const result: PieDetail = await fetcher(`${url}/pies/${encodeURIComponent(id)}`);
        if (!current(epoch, version)) return;
        if (result.id !== id) throw new Error('Malformed broker response: requested and returned Pie IDs differ. Mutations are disabled.');
        setDetail(result); setDetailStale(false);
      } else if (kind === 'search') {
        const result: TradingInstrument[] = await fetcher(`${url}/instruments?query=${encodeURIComponent(query.trim())}`);
        if (!current(epoch, version)) return;
        setMatches(result); setSearched(true);
      }
    } catch (error) {
      if (!current(epoch, version)) return;
      setReadError(error instanceof Error ? error.message : failureMessage(error));
      if (kind === 'list') setListStale(true);
      if (kind === 'detail') setDetailStale(true);
    } finally {
      if (current(epoch, version)) {
        reading.current = false;
        setReadBusy(null);
      }
    }
  };
  const startDraft = (action: Draft['action']) => {
    if (disabled || reading.current || (action !== 'create' && (!detail || detailStale || listStale))) return;
    const settings = action === 'create' ? null : detail?.settings;
    setDraft({ action, key: crypto.randomUUID(), title: '', note: '', source: action === 'create' ? null : detail,
      name: action === 'duplicate' ? '' : settings?.name ?? '', icon: action === 'duplicate' ? '' : settings?.icon ?? '',
      goal: settings?.goal ?? '', endDate: settings?.end_date ?? '', dividend: settings?.dividend_cash_action ?? 'REINVEST',
      allocations: settings?.instrument_shares ? Object.entries(settings.instrument_shares).map(([ticker, weight]) => ({ ticker, weight: weight ?? '' })) : [] });
    setFormError(null); setUncertain(false); setMatches([]); setSearched(false); setQuery('');
  };
  const editDraft = (change: Partial<Draft>) => {
    setDraft(previous => previous ? { ...previous, ...change, key: crypto.randomUUID() } : null);
    setUncertain(false); setFormError(null);
  };
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!draft || disabled || reading.current || (draft.action !== 'create' && (detailStale || listStale))) return;
    try {
      if (!draft.title.trim()) throw new Error('Enter a proposal title.');
      let operation: PieOperationInput;
      const old = draft.source?.settings;
      if (draft.action === 'delete') {
        if (!draft.source || !emptyHoldings(draft.source)) throw new Error('Deletion requires complete evidence of zero instrument holdings. Empty the Pie in Trading 212 and refresh first.');
        operation = { action: 'delete', pie_id: draft.source.id };
      } else if (draft.action === 'duplicate') {
        if (!draft.name.trim()) throw new Error('Enter the new copy name.');
        operation = { action: 'duplicate', pie_id: draft.source!.id, name: draft.name, ...(draft.icon ? { icon: draft.icon } : {}) };
      } else {
        const targets = Object.fromEntries(draft.allocations.map(item => [item.ticker, item.weight]));
        const replacingTargets = draft.action === 'create' || !sameTargets(old?.instrument_shares ?? null, targets);
        if (replacingTargets) {
          if (draft.allocations.length < 1 || draft.allocations.length > 50) throw new Error('Choose between 1 and 50 catalog STOCK/ETF instruments.');
          if (new Set(draft.allocations.map(item => item.ticker)).size !== draft.allocations.length) throw new Error('Every ticker must be distinct.');
          let total = 0n;
          for (const { ticker, weight } of draft.allocations) {
            if (!ticker || !/^(?:0|1)(?:\.\d{1,4})?$/.test(weight)) throw new Error('Each weight must be a positive fractional decimal with at most four decimal places.');
            const [whole, fraction = ''] = weight.split('.');
            const units = BigInt(whole) * 10000n + BigInt(fraction.padEnd(4, '0'));
            if (units <= 0n || units > 10000n) throw new Error('Each weight must be greater than 0 and at most 1.');
            total += units;
          }
          if (total !== 10000n) throw new Error('Weights must sum to exactly 1. No normalization or rounding is applied.');
        }
        if (!draft.name.trim()) throw new Error('Enter a nonblank Pie name.');
        if (draft.goal && (draft.action === 'create' || draft.goal !== old?.goal)) {
          const goal = decimalParts(draft.goal);
          if (!goal || goal.coefficient < 0n) throw new Error('Goal must be a nonnegative decimal string.');
        }
        if (draft.endDate && (draft.action === 'create' || draft.endDate !== old?.end_date) && (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/.test(draft.endDate) || Number.isNaN(Date.parse(draft.endDate)))) throw new Error('New end dates must be RFC3339 timestamps including a timezone.');
        if (draft.dividend !== 'REINVEST' && draft.dividend !== 'TO_ACCOUNT_CASH') throw new Error('Choose a supported dividend action.');
        if (draft.action === 'create') operation = { action: 'create', settings: { name: draft.name, dividend_cash_action: draft.dividend as DividendCashAction, instrument_shares: targets, ...(draft.icon ? { icon: draft.icon } : {}), ...(draft.goal ? { goal: draft.goal } : {}), ...(draft.endDate ? { end_date: draft.endDate } : {}) } };
        else {
          const changes: PieSettingsChanges = {};
          if (draft.name !== old?.name) changes.name = draft.name;
          if (draft.dividend !== old?.dividend_cash_action) changes.dividend_cash_action = draft.dividend;
          if (replacingTargets) changes.instrument_shares = targets;
          for (const [key, value, previous] of [['icon', draft.icon, old?.icon], ['goal', draft.goal, old?.goal], ['end_date', draft.endDate, old?.end_date]] as const) {
            if (value !== (previous ?? '')) {
              if (!value) throw new Error('Optional broker settings cannot be cleared: the broker does not document clearing semantics.');
              changes[key] = value;
            }
          }
          if (Object.keys(changes).length === 0) throw new Error('Change at least one setting or target weight.');
          if (draft.name === old?.name) throw new Error(`Trading 212 rejects Pie updates that keep the current name. Rename Pie #${draft.source!.id} to the next version, such as "${old?.name ?? 'Pie'} v1". Doughbox will not invent that version.`);
          operation = { action: 'update', pie_id: draft.source!.id, changes };
        }
      }
      setFormError(null);
      const epoch = getPrivateRequestEpoch(), version = requestVersion.current;
      const success = await onSubmit({ client_request_id: draft.key, title: draft.title, note: draft.note, operation });
      if (!current(epoch, version)) return;
      if (success) { setDraft(null); setUncertain(false); }
      else setUncertain(true);
    } catch (error) { setFormError(error instanceof Error ? error.message : failureMessage(error)); }
  };
  const blocked = disabled || readBusy !== null;
  const sourceBlocked = blocked || !detail || detailStale || listStale;
  return <div className="space-y-4">
    <Card><CardHeader><CardTitle>Trading 212 Pies</CardTitle><CardDescription>Doughbox supports Pies mainly for being compatible with existing Pies for Agentic Trading &amp; Plan creation. Brokerage imports are independent of Trading 212 Pies. Also, the Trading 212 Pie endpoints are marked as deprecated in their spec.</CardDescription></CardHeader>
      <CardContent className="space-y-3">
        <div className="flex flex-wrap gap-2"><Button type="button" variant="outline" disabled={blocked} onClick={() => read('list')}>{readBusy === 'list' ? 'Loading Pies…' : list ? 'Refresh Pie list' : 'Load Pies'}</Button><Button type="button" disabled={blocked} onClick={() => startDraft('create')}>Create Pie proposal</Button></div>
        {readBusy && <p role="status" className="text-sm">{readBusy === 'detail' ? 'Loading selected Pie…' : readBusy === 'search' ? 'Searching saved instrument catalog…' : 'Loading broker Pie list…'}</p>}
        {readError && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{readError}</p>}
        {listStale && <p role="alert" className="text-sm text-amber-800 dark:text-amber-200">Pie list is stale. Refresh the list successfully before preparing changes to existing Pies.</p>}
        {list ? <>
          <p className="text-xs text-muted-foreground">Updated <time dateTime={list.fetched_at}>{new Date(list.fetched_at).toLocaleString()}</time> · {currency ? `Account currency: ${currency}` : 'Account currency not provided'}</p>
          {list.items.length === 0 ? <p className="text-sm">No Pies returned.</p> : <ul className="grid gap-4 sm:grid-cols-2">
            {list.items.map(pie => {
              const active = selected === pie.id;
              const name = detail?.id === pie.id ? detail.settings?.name : null;
              const result = pie.result?.price_avg_result;
              const change = result == null ? null : decimalParts(result)?.coefficient;
              const resultColor = change == null || change === 0n ? 'text-muted-foreground' : change < 0n ? 'text-destructive-foreground' : 'text-success-foreground';
              const status = ({ AHEAD: 'Ahead of goal', ON_TRACK: 'On track', BEHIND: 'Behind goal' } as Record<string, string>)[pie.status ?? ''] ?? pie.status ?? 'Status unknown';
              return <li key={pie.id} className={`min-w-0 overflow-hidden rounded-xl border bg-card transition-colors ${active ? 'border-primary ring-1 ring-primary' : 'border-border'}`}>
                <button type="button" aria-label={`View Pie #${pie.id}`} aria-pressed={active} disabled={blocked || listStale}
                  className={`group flex w-full items-center gap-3 p-4 text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 ${active ? 'bg-primary/5' : ''}`}
                  onClick={() => {
                    setSelected(pie.id);
                    setDetail(null);
                    setDetailStale(true);
                    void read('detail', pie.id);
                  }}>
                  <span className={`flex size-10 shrink-0 items-center justify-center rounded-lg ${active ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground'}`}><ChartPieSliceIcon aria-hidden="true" className="size-5" /></span>
                  <span className="min-w-0 flex-1">
                    <span className="block break-words font-semibold">{name ?? `Pie #${pie.id}`}</span>
                    <span className="block break-all text-xs text-muted-foreground">{name ? `Broker ID ${pie.id}` : 'View settings and holdings'}</span>
                  </span>
                  <ArrowRightIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
                </button>
                <div className="space-y-4 p-4 pt-3">
                  <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                    <Badge variant="outline" className="font-normal">{status}</Badge>
                    <span className="text-muted-foreground">Goal progress <span className="font-medium tabular-nums text-foreground">{summaryPercentage(pie.progress)}</span></span>
                  </div>
                  <dl className="grid grid-cols-2 gap-4">
                    <PieMetric prominent label={`Current value${currency ? ` · ${currency}` : ''}`} value={summaryNumber(pie.result?.price_avg_value)} title={pie.result?.price_avg_value ?? undefined} />
                    <PieMetric prominent tone={resultColor} label={`Return${currency ? ` · ${currency}` : ''}`} value={summaryNumber(result)} title={result ?? undefined} hint={pie.result?.price_avg_result_coef == null ? undefined : summaryPercentage(pie.result.price_avg_result_coef)} />
                  </dl>
                  <dl className="grid grid-cols-2 gap-x-4 gap-y-3 border-t pt-3 text-xs">
                    <PieMetric label="Invested value" value={`${summaryNumber(pie.result?.price_avg_invested_value)}${pie.result?.price_avg_invested_value != null && currency ? ` ${currency}` : ''}`} title={pie.result?.price_avg_invested_value ?? undefined} />
                    <PieMetric label="Amount added · not free cash" labelTitle="Broker-reported amount put into the Pie, not its available free cash." title={pie.cash ?? undefined} value={`${summaryNumber(pie.cash)}${pie.cash != null && currency ? ` ${currency}` : ''}`} />
                  </dl>
                  {pie.dividend_details && <div className="space-y-2 rounded-lg bg-muted/40 p-3">
                    <p className="text-xs font-medium">Dividends{currency ? ` · ${currency}` : ''}</p>
                    <dl className="grid grid-cols-3 gap-2 text-xs">
                      <PieMetric label="Earned" value={summaryNumber(pie.dividend_details.gained)} title={pie.dividend_details.gained ?? undefined} />
                      <PieMetric label="In cash" value={summaryNumber(pie.dividend_details.in_cash)} title={pie.dividend_details.in_cash ?? undefined} />
                      <PieMetric label="Reinvested" value={summaryNumber(pie.dividend_details.reinvested)} title={pie.dividend_details.reinvested ?? undefined} />
                    </dl>
                  </div>}
                </div>
              </li>;
            })}
          </ul>}
        </> : <p className="text-sm text-muted-foreground">Load the list, then select a Pie to fetch its name, settings and holdings. The list itself does not contain names.</p>}
      </CardContent>
    </Card>
    {selected && <Card>
      <CardHeader>
        <CardTitle className="break-all">{detail?.settings?.name ?? `Pie #${selected}`}</CardTitle>
        <CardDescription className="break-all">Exact broker ID: {selected}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <Button type="button" variant="outline" disabled={blocked} onClick={() => read('detail', selected)}>Refresh selected Pie</Button>
        {detailStale && <p role="alert" className="text-sm text-amber-800 dark:text-amber-200">Detail is stale or unavailable. Refresh successfully before preparing a source-dependent mutation.</p>}
        {detail && <>
          <p className="text-xs text-muted-foreground">Fetched <time dateTime={detail.fetched_at}>{detail.fetched_at}</time></p>
          <Settings label="Writable configuration" settings={detail.settings} currency={currency} />
          <Holdings detail={detail} currency={currency} />
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="outline" disabled={sourceBlocked || !writable(detail)} onClick={() => startDraft('update')}>Edit settings / allocation</Button>
            <Button type="button" variant="outline" disabled={sourceBlocked || !writable(detail)} onClick={() => startDraft('duplicate')}>Duplicate composition</Button>
            <Button type="button" variant="destructive" disabled={sourceBlocked || !writable(detail) || !emptyHoldings(detail)} onClick={() => startDraft('delete')}>Delete Pie proposal</Button>
          </div>
          {!writable(detail) && <p className="text-sm text-muted-foreground">The broker returned incomplete writable settings. Changes cannot be safely prepared.</p>}
          {!emptyHoldings(detail) && <p className="text-sm text-muted-foreground">Deletion is blocked: holdings are nonzero or quantities are unknown. Empty investments and Pie cash in Trading 212, then refresh. Reported cash does not independently prove available Pie cash.</p>}
        </>}
      </CardContent>
    </Card>}
    {draft && <Card><CardHeader><CardTitle>{({ create: 'Create Pie', update: 'Edit Pie settings / allocation', duplicate: 'Duplicate composition', delete: 'Delete Pie' })[draft.action]} — proposal only</CardTitle><CardDescription>{draft.source ? `Source Pie ID ${draft.source.id}. ` : ''}No broker mutation occurs during preparation. Review and explicitly approve in Orders afterwards.</CardDescription></CardHeader><CardContent>
      <form className="space-y-4" onSubmit={submit}>
        <fieldset disabled={blocked} className="min-w-0 space-y-4">
          <div className="space-y-2"><Label htmlFor="pie-title">Proposal title</Label><Input id="pie-title" required maxLength={200} value={draft.title} onChange={event => editDraft({ title: event.target.value })} /></div>
          <div className="space-y-2"><Label htmlFor="pie-note">Review note (optional)</Label><Input id="pie-note" maxLength={4000} value={draft.note} onChange={event => editDraft({ note: event.target.value })} /></div>
          {draft.action !== 'delete' && <>
            <div className="grid gap-3 sm:grid-cols-2"><div className="space-y-2"><Label htmlFor="pie-name">{draft.action === 'duplicate' ? 'New copy name' : 'Pie name'}</Label><Input id="pie-name" required value={draft.name} onChange={event => editDraft({ name: event.target.value })} /></div><div className="space-y-2"><Label htmlFor="pie-icon">Icon (optional broker value)</Label><Input id="pie-icon" value={draft.icon} onChange={event => editDraft({ icon: event.target.value })} /></div></div>
            {draft.action === 'duplicate' ? <p className="text-sm">Only composition is copied. Holdings, AutoInvest and goals are not inherited. No funding or purchases.</p> : <>
              <div className="space-y-2"><Label htmlFor="pie-dividend">Dividend cash action</Label><select id="pie-dividend" className="h-10 w-full rounded-md border bg-background px-3 text-sm" value={draft.dividend} onChange={event => editDraft({ dividend: event.target.value })}><option value="REINVEST">REINVEST</option><option value="TO_ACCOUNT_CASH">TO_ACCOUNT_CASH</option>{!['REINVEST', 'TO_ACCOUNT_CASH'].includes(draft.dividend) && <option value={draft.dividend}>{draft.dividend}</option>}</select></div>
              <div className="grid gap-3 sm:grid-cols-2"><div className="space-y-2"><Label htmlFor="pie-goal">Goal ({currency ?? 'native account currency'}; optional)</Label><Input id="pie-goal" inputMode="decimal" value={draft.goal} onChange={event => editDraft({ goal: event.target.value })} /></div><div className="space-y-2"><Label htmlFor="pie-end-date">End date (RFC3339; optional)</Label><Input id="pie-end-date" placeholder="2030-01-01T00:00:00Z" value={draft.endDate} onChange={event => editDraft({ endDate: event.target.value })} /></div></div>
              <p className="text-xs text-muted-foreground">Unchanged optional settings are preserved exactly. Clearing them is not supported. Allocation edits replace the entire target map and do not buy or sell shares.{draft.action === 'update' ? ' Trading 212 rejects an update that keeps the current name. Use the next version: Name v1, then Name v2. Doughbox will not invent that version.' : ''}</p>
              <section className="space-y-3"><h4 className="font-medium">Target allocations</h4><p className="text-sm">Weight (0–1; total 1). Use exact fractional decimal strings, at most four decimal places. No rounding or normalization.</p>
                <ul className="space-y-2">{draft.allocations.map((item, index) => <li key={`${item.ticker}-${index}`} className="flex flex-wrap items-end gap-2 rounded-md border p-2"><div className="min-w-0 flex-1 space-y-1"><Label htmlFor={`pie-weight-${index}`} className="break-all">{item.ticker} · Weight (0–1; total 1)</Label><Input id={`pie-weight-${index}`} inputMode="decimal" value={item.weight} onChange={event => editDraft({ allocations: draft.allocations.map((existing, itemIndex) => itemIndex === index ? { ...existing, weight: event.target.value } : existing) })} /></div><output className="py-2 text-sm tabular-nums" htmlFor={`pie-weight-${index}`}>{piePercentage(item.weight)}</output><Button type="button" variant="outline" onClick={() => editDraft({ allocations: draft.allocations.filter((_, itemIndex) => itemIndex !== index) })}>Remove</Button></li>)}</ul>
                <div className="space-y-2"><Label htmlFor="pie-instrument-query">Find a STOCK/ETF in the connected instrument catalog</Label><div className="flex gap-2"><Input id="pie-instrument-query" maxLength={100} value={query} onChange={event => { setQuery(event.target.value); setMatches([]); setSearched(false); }} /><Button type="button" variant="outline" disabled={!query.trim() || query.trim().length > 100} onClick={() => read('search')}>Search</Button></div><p className="text-xs text-muted-foreground">Up to 50 matches; no live metadata requests. The server validates catalog eligibility when preparing the proposal.</p></div>
                {searched && matches.length === 0 && <p className="text-sm">No matching catalog instruments.</p>}
                <ul className="space-y-2">{matches.map(instrument => <li key={instrument.ticker} className="flex flex-wrap items-center justify-between gap-2 rounded-md border p-2 text-sm"><div className="min-w-0 break-words"><p>{instrument.name} · <code>{instrument.ticker}</code></p><p className="text-xs text-muted-foreground">{instrument.currency} · ISIN {instrument.isin}</p></div><Button type="button" variant="outline" disabled={draft.allocations.length >= 50 || draft.allocations.some(item => item.ticker === instrument.ticker)} onClick={() => editDraft({ allocations: [...draft.allocations, { ticker: instrument.ticker, weight: '' }] })}>Add</Button></li>)}</ul>
              </section>
              {draft.action === 'create' && <p className="text-sm font-medium">Creates a configuration only. No funding or shares are purchased.</p>}
            </>}
          </>}
          {draft.action === 'delete' && <p className="text-sm font-medium text-red-700 dark:text-red-300">Deletion is permanent. Complete zero-holdings evidence is required, but free Pie cash is not independently established. The owner must separately acknowledge empty investments and Pie cash when approving in Orders.</p>}
        </fieldset>
        {formError && <p role="alert" className="text-sm text-red-700 dark:text-red-300">{formError}</p>}
        {uncertain && <p role="alert" className="text-sm text-amber-800 dark:text-amber-200">Proposal creation was not fully confirmed. Your draft and request key are retained. An explicit retry of unchanged intent uses the same key; editing any intent creates a new key. Check Orders before replacing an uncertain request.</p>}
        <div className="flex flex-wrap gap-2"><Button type="submit" disabled={blocked || (draft.action !== 'create' && (detailStale || listStale))}>{busyAction === 'pie-proposal' ? 'Preparing…' : uncertain ? 'Retry same proposal' : 'Prepare proposal for review'}</Button><Button type="button" variant="outline" disabled={blocked} onClick={() => { setDraft(null); setFormError(null); setUncertain(false); }}>Cancel draft</Button></div>
      </form>
    </CardContent></Card>}
  </div>;
}
