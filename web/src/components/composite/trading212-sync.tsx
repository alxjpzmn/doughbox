import { useEffect, useRef, useState } from 'react';
import { Link } from 'wouter';
import useSWR, { useSWRConfig } from 'swr';
import { BASE_URL, fetcher, getPrivateRequestEpoch, isCurrentPrivateRequestEpoch, sendMutateRequest, type HttpError } from '@/lib/http';
import { isImportedFinancialCacheKey } from '@/lib/import-cache';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import type { T212ReviewCandidate, T212ReviewDecision, T212ReviewItem, T212ReviewPage, T212SyncAccount, T212SyncRun, T212SyncStatus } from '@/types/trading';

export const tradingSyncUrl = `${BASE_URL}/trading/sync`;
const setupHref = '~/dashboard/trading?section=setup#t212-history-sync';
const activeStatuses: Record<string, true> = { queued: true, running: true, waiting: true, needs_review: true };
const phaseLabels: Record<T212SyncRun['phase'], string> = {
  request_report: 'Requesting report',
  poll_report: 'Waiting for report',
  download_report: 'Downloading report',
  import_report: 'Importing report',
  history_orders: 'Reading order history',
  history_dividends: 'Reading dividend history',
  history_transactions: 'Reading cash history',
};

function message(error: unknown): string {
  return error && typeof error === 'object' && 'message' in error && typeof error.message === 'string'
    ? error.message : 'Trading 212 sync is unavailable. Retry when the connection recovers.';
}
function when(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : 'Not recorded';
}
export function tradingAccountLabel(account: Pick<T212SyncAccount, 'environment' | 'broker_account_id' | 'currency'>, missingCurrency = ''): string {
  const kind = account.environment === 'demo' ? 'Demo' : 'Live';
  const currency = account.currency || missingCurrency;
  return currency ? `${kind} · ${account.broker_account_id} · ${currency}` : `${kind} · ${account.broker_account_id}`;
}
function counts(run: { inserted_count: number; matched_count: number; review_count: number; skipped_count: number }): string {
  return `${run.inserted_count} inserted · ${run.matched_count} already imported · ${run.review_count} review · ${run.skipped_count} skipped`;
}


export function useTradingSync() {
  const { mutate: mutateCache } = useSWRConfig();
  const [online, setOnline] = useState(() => navigator.onLine);
  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener('online', update);
    window.addEventListener('offline', update);
    return () => {
      window.removeEventListener('online', update);
      window.removeEventListener('offline', update);
    };
  }, []);
  const result = useSWR<T212SyncStatus, HttpError>(tradingSyncUrl, fetcher, {
    refreshInterval: current => current?.runs.some(run => activeStatuses[run.status] && run.status !== 'needs_review') ? 2_000 : 60_000,
  });
  const seen = useRef<string | null>(null);
  const [financialRefreshError, setFinancialRefreshError] = useState<string | null>(null);
  useEffect(() => {
    if (!result.data) return;
    const identity = JSON.stringify(result.data.runs
      .filter(run => run.inserted_count > 0 || run.matched_count > 0)
      .map(run => [run.id, run.status, run.inserted_count, run.matched_count, run.review_count]));
    if (seen.current === identity) return;
    seen.current = identity;
    if (identity === '[]') return;
    let cancelled = false;
    const epoch = getPrivateRequestEpoch();
    setFinancialRefreshError(null);
    void mutateCache(isImportedFinancialCacheKey).catch((error: unknown) => {
      if (!cancelled && isCurrentPrivateRequestEpoch(epoch)) setFinancialRefreshError(`Portfolio data could not be refreshed. Reload to see current data. ${message(error)}`);
    });
    return () => { cancelled = true; };
  }, [result.data, mutateCache]);
  return { ...result, online, financialRefreshError };
}

export function Trading212SyncSummary() {
  const { data, error, online, financialRefreshError } = useTradingSync();
  const account = data?.accounts.find(item => item.id === data.current_account_id) ?? null;
  const review = data?.review_batches.reduce((total, batch) => total + batch.review_count, 0) ?? 0;
  return (
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
      <div className="min-w-0 space-y-1">
        <p className="text-sm font-medium">{account ? `Trading 212 ${account.environment === 'demo' ? 'demo' : 'live'} history` : 'Trading 212 history'}</p>
        <p className="text-xs text-muted-foreground">
          {!online ? 'Offline — sync status is not current.'
            : error ? 'Sync status could not be refreshed.'
            : !data ? 'Loading sync status…'
            : !account ? 'No verified Trading 212 account is connected.'
            : !data.available ? data.unavailable_reason || 'Trading 212 sync is unavailable.'
            : account.schedule_enabled ? `Daily sync ${account.next_run_at && new Date(account.next_run_at) <= new Date() ? 'due now' : `next ${when(account.next_run_at)}`}` : 'Daily sync paused'}
          {review > 0 ? ` · ${review} match${review === 1 ? '' : 'es'} to review` : ''}
          {financialRefreshError && <span role="alert"> · {financialRefreshError}</span>}
        </p>
      </div>
      <Button asChild variant="outline" size="sm"><Link href={setupHref}>Manage sync</Link></Button>
    </div>
  );
}

export function Trading212SyncPanel() {
  const { mutate: mutateCache } = useSWRConfig();
  const { data, error, online, mutate, financialRefreshError } = useTradingSync();
  const mounted = useRef(false);
  const submitting = useRef(false);
  const [draftEnabled, setDraftEnabled] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [acting, setActing] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmRescan, setConfirmRescan] = useState(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const account = data?.accounts.find(item => item.id === data.current_account_id) ?? null;
  const active = data?.runs.find(run => activeStatuses[run.status]) ?? null;
  const stale = !!error || !online;
  const brokerDisabled = stale || !data?.available || saving || acting !== null;
  const current = (epoch: number) => mounted.current && isCurrentPrivateRequestEpoch(epoch);

  const saveSchedule = async () => {
    if (draftEnabled === null || brokerDisabled || submitting.current) return;
    submitting.current = true;
    const epoch = getPrivateRequestEpoch();
    setSaving(true);
    setFailure(null);
    try {
      await sendMutateRequest(tradingSyncUrl + '/scheduler', { enabled: draftEnabled }, { method: 'PUT' });
      if (!current(epoch)) return;
      await mutate();
      if (!current(epoch)) return;
      setDraftEnabled(null);
      setNotice(draftEnabled ? 'Daily sync enabled. The first run is due now and continues if you leave this page.' : 'Future scheduled runs are paused. A run already started will finish.');
    } catch (error: unknown) {
      if (current(epoch) && !(error instanceof Error && error.name === 'AbortError')) setFailure(message(error));
    } finally {
      submitting.current = false;
      if (current(epoch)) setSaving(false);
    }
  };
  const requestRun = async (fullHistory: boolean) => {
    if (brokerDisabled || submitting.current) return;
    submitting.current = true;
    const epoch = getPrivateRequestEpoch();
    setActing(fullHistory ? 'rescan' : 'sync');
    setFailure(null);
    setConfirmRescan(false);
    try {
      await sendMutateRequest(tradingSyncUrl + '/runs', { full_history: fullHistory });
      if (!current(epoch)) return;
      setNotice(fullHistory ? 'Full-history rescan queued. Existing ledger rows and provenance are retained.' : 'Sync queued. It continues after you close this tab.');
      await mutate();
    } catch (error: unknown) {
      if (current(epoch) && !(error instanceof Error && error.name === 'AbortError')) setFailure(message(error));
    } finally {
      submitting.current = false;
      if (current(epoch)) setActing(null);
    }
  };

  return (
    <Card id="t212-history-sync">
      <CardHeader>
        <CardTitle>Live history sync</CardTitle>
        <CardDescription>Imports the connected demo or live account. Daily means one rolling 24-hour run plus Sync now, not streaming. Manual CSV imports remain available and use the same ledger.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {!data && !error && <p role="status" className="text-sm text-muted-foreground">Loading live sync status…</p>}
        {!online && <p role="alert" className="text-sm text-red-700 dark:text-red-300">Offline. Broker and review controls are disabled until the connection returns.</p>}
        {error && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">Sync status could not be refreshed. Broker controls are disabled until it reconnects. {data ? 'Last loaded status is shown. ' : ''}{message(error)}</p>}
        {data && !data.available && <p className="text-sm text-muted-foreground">{data.unavailable_reason}</p>}
        {account && <div className="space-y-1 text-sm">
          <p>{account.environment === 'demo' ? 'Demo' : 'Live'} account <span className="break-all font-medium">{account.broker_account_id}</span>{account.currency ? ` · ${account.currency}` : ''}</p>
          <p className="text-xs text-muted-foreground">Covered through {when(account.synced_through)} · Last success {when(account.last_success_at)} · {account.backfill_complete ? 'Initial history complete' : `Backfill cursor ${when(account.backfill_cursor)}`}</p>
        </div>}
        {data && draftEnabled === null ? <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
          <div>
            <p className="text-sm font-medium">Daily (every 24 hours)</p>
            <p className="text-xs text-muted-foreground">{account?.schedule_enabled ? `Next run ${when(account.next_run_at)}` : 'Paused. Enabling starts the first run immediately.'}</p>
          </div>
          <Button type="button" variant="outline" size="sm" disabled={!account || brokerDisabled} onClick={() => { setDraftEnabled(account?.schedule_enabled ?? false); setFailure(null); }}>Manage scheduler</Button>
        </div> : data && <form className="space-y-3 rounded-lg border p-3" onSubmit={event => { event.preventDefault(); void saveSchedule(); }}>
          <div className="flex items-center gap-3">
            <Switch id="t212-daily-sync" checked={draftEnabled ?? false} disabled={brokerDisabled} onCheckedChange={setDraftEnabled} />
            <Label htmlFor="t212-daily-sync">Daily (every 24 hours)</Label>
          </div>
          <p className="text-xs text-muted-foreground">Pausing stops future scheduled claims, not a run already started. Saving the same setting keeps its deadline. Downtime creates one catch-up run, not one per missed day.</p>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" size="sm" disabled={brokerDisabled}>{saving ? 'Saving…' : 'Save schedule'}</Button>
            <Button type="button" variant="outline" size="sm" disabled={saving} onClick={() => { setDraftEnabled(null); setFailure(null); }}>Cancel</Button>
          </div>
        </form>}
        {active && <div role="status" className="space-y-1 rounded-lg border p-3 text-sm">
          <p className="font-medium">{active.status === 'waiting' ? 'Waiting' : active.status === 'needs_review' ? 'Review required' : 'Sync in progress'} · {phaseLabels[active.phase]}</p>
          <p className="text-xs text-muted-foreground">{counts(active)}{active.error_message ? ` · ${active.error_message}` : ''}</p>
          {active.status === 'waiting' && <p className="text-xs text-muted-foreground">Next attempt {when(active.next_attempt_at)}. This does not block trading execution.</p>}
        </div>}
        <div className="flex flex-wrap gap-2">
          <Button type="button" disabled={brokerDisabled} onClick={() => void requestRun(false)}>{acting === 'sync' ? 'Queuing…' : 'Sync now'}</Button>
          {confirmRescan
            ? <Button type="button" variant="outline" disabled={brokerDisabled} onClick={() => void requestRun(true)}>{acting === 'rescan' ? 'Queuing…' : 'Confirm rescan'}</Button>
            : <Button type="button" variant="outline" disabled={brokerDisabled} onClick={() => setConfirmRescan(true)}>Rescan all history</Button>}
          <Button type="button" variant="outline" onClick={() => document.getElementById('t212-review-matches')?.scrollIntoView()}>Review matches</Button>
        </div>
        {confirmRescan && <p className="text-xs text-muted-foreground">Rescan reads history again from the beginning without deleting ledger data or resetting provenance. It does not run while another sync is active.</p>}
        {failure && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{failure}</p>}
        {financialRefreshError && <p role="alert" className="text-sm text-red-700 dark:text-red-300">{financialRefreshError}</p>}
        {notice && <p role="status" className="text-sm">{notice}</p>}
        {data && data.runs.length > 0 && <ul aria-label="Latest Trading 212 sync runs" className="max-h-72 space-y-2 overflow-y-auto">
          {data.runs.slice(0, 20).map(run => <li key={run.id} className="rounded-lg border p-3 text-sm">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium">{run.trigger === 'full_history' ? 'Rescan all history' : run.trigger === 'scheduled' ? 'Daily sync' : 'Sync now'}</span>
              <Badge variant="outline">{run.status}</Badge>
            </div>
            <p className="mt-1 text-xs text-muted-foreground">{when(run.created_at)} · {counts(run)}</p>
            {run.error_message && <p className="mt-1 break-words text-xs text-red-700 dark:text-red-300">{run.error_message}</p>}
          </li>)}
        </ul>}
        <ReviewMatches accounts={data?.accounts ?? []} batches={data?.review_batches} stale={stale} onApplied={async () => {
          const epoch = getPrivateRequestEpoch();
          await mutate();
          if (current(epoch)) await mutateCache(isImportedFinancialCacheKey);
        }} />
      </CardContent>
    </Card>
  );
}



function ReviewMatches({ accounts, batches, stale, onApplied }: {
  accounts: T212SyncAccount[];
  batches: T212SyncStatus['review_batches'] | undefined;
  stale: boolean;
  onApplied: () => Promise<void>;
}) {
  const [items, setItems] = useState<T212ReviewItem[]>([]);
  const [nextCursors, setNextCursors] = useState<Record<string, number>>({});
  const [accountsByBatch, setAccountsByBatch] = useState<Record<string, string>>({});
  const [selected, setSelected] = useState<Record<string, { action: 'match_existing' | 'import_new'; ledgerId: string; accept: boolean }>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [loading, setLoading] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const mounted = useRef(false);
  const submittingRef = useRef(false);
  const batchesRef = useRef(batches);
  batchesRef.current = batches;
  const batchKey = batches?.map(batch => `${batch.id}:${batch.review_count}:${batch.account_id ?? ''}`).join('|') ?? '';
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let cancelled = false;
    const epoch = getPrivateRequestEpoch();
    const currentBatches = batchesRef.current ?? [];
    setLoading(true);
    setSelected({});
    setItems([]);
    setFailure(null);
    void (async () => {
      const loaded: T212ReviewItem[] = [];
      const cursors: Record<string, number> = {};
      for (const batch of currentBatches) {
        const page: T212ReviewPage = await fetcher(`${tradingSyncUrl}/reviews?batch_id=${encodeURIComponent(batch.id)}&limit=50`);
        loaded.push(...page.items);
        if (page.next_cursor !== null) cursors[batch.id] = page.next_cursor;
      }
      if (cancelled || !mounted.current || !isCurrentPrivateRequestEpoch(epoch)) return;
      setItems(loaded);
      setNextCursors(cursors);
      setAccountsByBatch(Object.fromEntries(currentBatches.map(batch => [batch.id, batch.account_id ?? ''])));
      setUnavailable(false);
    })().catch((error: unknown) => {
      if (!cancelled && mounted.current && isCurrentPrivateRequestEpoch(epoch) && !(error instanceof Error && error.name === 'AbortError')) {
        setFailure(message(error));
        setUnavailable(true);
      }
    }).finally(() => {
      if (!cancelled && mounted.current && isCurrentPrivateRequestEpoch(epoch)) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [batchKey, refreshVersion]);

  const loadMore = async (batchId: string, cursor: number) => {
    if (loading || submittingRef.current) return;
    const epoch = getPrivateRequestEpoch();
    setLoading(true);
    try {
      const page: T212ReviewPage = await fetcher(`${tradingSyncUrl}/reviews?batch_id=${encodeURIComponent(batchId)}&cursor=${cursor}&limit=50`);
      if (!mounted.current || !isCurrentPrivateRequestEpoch(epoch)) return;
      setItems(current => [...current, ...page.items.filter(item => !current.some(existing => existing.batch_id === item.batch_id && existing.row_number === item.row_number))]);
      setNextCursors(current => {
        const next = { ...current };
        if (page.next_cursor === null) delete next[batchId];
        else next[batchId] = page.next_cursor;
        return next;
      });
    } catch (error: unknown) {
      if (mounted.current && isCurrentPrivateRequestEpoch(epoch) && !(error instanceof Error && error.name === 'AbortError')) {
        setFailure(message(error));
        setUnavailable(true);
      }
    } finally {
      if (mounted.current && isCurrentPrivateRequestEpoch(epoch)) setLoading(false);
    }
  };
  const selectedCount = Object.keys(selected).length;
  const acknowledgmentMissing = items.some(item => {
    const choice = selected[`${item.batch_id}:${item.row_number}`];
    if (!choice || choice.action !== 'match_existing') return false;
    const candidate = item.candidates.find(candidate => candidate.ledger_id === choice.ledgerId);
    return !candidate || (candidate.values_differ && !choice.accept);
  });
  const disabled = stale || loading || unavailable || submitting;
  const submit = async () => {
    if (disabled || submittingRef.current || acknowledgmentMissing || selectedCount === 0 || selectedCount > 200) return;
    const decisions: T212ReviewDecision[] = [];
    for (const item of items) {
      const choice = selected[`${item.batch_id}:${item.row_number}`];
      if (!choice) continue;
      const location = { batch_id: item.batch_id, row_number: item.row_number, version: item.version, account_id: item.account_id ?? (accountsByBatch[item.batch_id] || null) };
      decisions.push(choice.action === 'import_new'
        ? { ...location, action: 'import_new' }
        : { ...location, action: 'match_existing', kind: item.kind, ledger_id: choice.ledgerId, accept_recorded_values: choice.accept });
    }
    const epoch = getPrivateRequestEpoch();
    submittingRef.current = true;
    setSubmitting(true);
    setFailure(null);
    try {
      await sendMutateRequest(`${tradingSyncUrl}/reviews`, { decisions });
      if (!mounted.current || !isCurrentPrivateRequestEpoch(epoch)) return;
      setSelected({});
      await onApplied();
      if (mounted.current && isCurrentPrivateRequestEpoch(epoch)) setRefreshVersion(version => version + 1);
    } catch (error: unknown) {
      if (mounted.current && isCurrentPrivateRequestEpoch(epoch) && !(error instanceof Error && error.name === 'AbortError')) setFailure(message(error));
    } finally {
      submittingRef.current = false;
      if (mounted.current && isCurrentPrivateRequestEpoch(epoch)) setSubmitting(false);
    }
  };
  return (
    <section id="t212-review-matches" aria-label="Trading 212 match review" className="space-y-3">
      <h3 className="text-sm font-medium">Review matches</h3>
      <p className="text-xs text-muted-foreground">Ambiguous historical rows stay unresolved until you explicitly choose. Linking keeps recorded amounts unchanged. Select up to 200 decisions for one confirmation; unselected rows remain unresolved.</p>
      {batches?.map(batch => <p key={batch.id} className="text-xs text-muted-foreground">
        Batch {batch.id} · {counts(batch)}
        {Object.keys(batch.skipped_actions).length > 0 && ` · Skipped actions: ${Object.entries(batch.skipped_actions).map(([action, count]) => `${action} (${count})`).join(', ')}`}
      </p>)}
      <div className="flex flex-wrap gap-2">
        <Button type="button" size="sm" variant="outline" disabled={disabled || !items.some(item => item.candidates.length === 1 && !item.candidates[0].values_differ && (item.candidates[0].import_source === null || item.candidates[0].import_source === 'legacy'))} onClick={() => {
          const exact = items.filter(item => item.candidates.length === 1 && !item.candidates[0].values_differ && (item.candidates[0].import_source === null || item.candidates[0].import_source === 'legacy'));
          setSelected(current => {
            const next = { ...current };
            for (const item of exact) {
              const key = `${item.batch_id}:${item.row_number}`;
              if (!next[key] && Object.keys(next).length >= 200) break;
              next[key] = { action: 'match_existing', ledgerId: item.candidates[0].ledger_id, accept: false };
            }
            return next;
          });
        }}>Select suggested exact legacy matches</Button>
        <Button type="button" size="sm" variant="outline" disabled={loading || submitting} onClick={() => setRefreshVersion(version => version + 1)}>Refresh review</Button>
      </div>
      {loading && <p role="status" className="text-sm text-muted-foreground">Loading match evidence…</p>}
      {!loading && (batches?.length ?? 0) === 0 && <p className="text-sm text-muted-foreground">No pending matches.</p>}
      {items.map(item => {
        const key = `${item.batch_id}:${item.row_number}`;
        const choice = selected[key];
        const candidate = item.candidates.find(entry => entry.ledger_id === choice?.ledgerId);
        const account = accounts.find(account => account.id === item.account_id);
        return <article key={key} className="space-y-3 rounded-lg border p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-sm font-medium">{item.raw_action} · {item.kind}</p>
            <Badge variant="outline">{item.source === 'trading212_api' ? 'API report' : 'CSV import'}</Badge>
          </div>
          <p className="text-xs text-muted-foreground">{item.reason}{item.raw_id ? ` · Broker ID ${item.raw_id}` : ' · No broker ID'}</p>
          {item.account_id ? <p className="text-xs text-muted-foreground">Statement account: {account ? tradingAccountLabel(account) : item.account_id}</p>
            : <div className="max-w-sm space-y-2">
              <Label htmlFor={`t212-review-account-${key}`}>Account for this statement batch</Label>
              <Select value={accountsByBatch[item.batch_id] || 'unassigned'} disabled={disabled} onValueChange={value => setAccountsByBatch(current => ({ ...current, [item.batch_id]: value === 'unassigned' ? '' : value }))}>
                <SelectTrigger id={`t212-review-account-${key}`}><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="unassigned">Unassigned — keep unknown ownership</SelectItem>
                  {accounts.map(account => <SelectItem key={account.id} value={account.id}>{tradingAccountLabel(account)}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>}
          <div className="grid gap-3 md:grid-cols-2">
            <ValueList title="Incoming native values" values={item.native} />
            <ValueList title="Existing native values" values={candidate?.native ?? {}} empty="No candidate selected" />
          </div>
          {item.candidates.length > 0 && <div className="space-y-2">
            {item.candidates.map(entry => {
              const recorded = accounts.find(account => account.id === entry.account_id);
              return <label key={entry.ledger_id} className="flex items-start gap-2 text-sm">
              <input type="radio" name={`candidate-${key}`} className="mt-1" disabled={disabled} checked={choice?.action === 'match_existing' && choice.ledgerId === entry.ledger_id} onChange={() => setSelected(current => ({ ...current, [key]: { action: 'match_existing', ledgerId: entry.ledger_id, accept: false } }))} />
              <span>Same transaction — keep recorded amounts. <CandidateLabel candidate={entry} /> · {entry.account_id ? `Recorded account ${recorded ? tradingAccountLabel(recorded) : entry.account_id}` : 'Recorded account unknown'}</span>
            </label>;
            })}
          </div>}
          {candidate && <details className="text-xs">
            <summary className="cursor-pointer">Compare saved accounting values</summary>
            <div className="mt-2 grid gap-3 md:grid-cols-2">
              <ValueList title="Incoming accounting" values={(item.normalized.event as { value?: Record<string, unknown> } | undefined)?.value ?? {}} />
              <ValueList title="Recorded accounting" values={(candidate.recorded.value as Record<string, unknown> | undefined) ?? {}} />
            </div>
          </details>}
          {candidate?.values_differ && <label className="flex items-start gap-2 text-sm">
            <input type="checkbox" className="mt-1" disabled={disabled} checked={choice?.accept ?? false} onChange={event => setSelected(current => ({ ...current, [key]: { action: 'match_existing', ledgerId: candidate.ledger_id, accept: event.target.checked } }))} />
            <span>Keep the recorded amounts. Linking does not overwrite fees, withholding, or other recorded values.</span>
          </label>}
          <div className="flex flex-wrap gap-2">
            <Button type="button" size="sm" disabled={disabled} variant={choice?.action === 'import_new' ? 'default' : 'outline'} onClick={() => setSelected(current => ({ ...current, [key]: { action: 'import_new', ledgerId: '', accept: false } }))}>Separate transaction</Button>
            <Button type="button" size="sm" disabled={disabled} variant="outline" onClick={() => setSelected(current => { const next = { ...current }; delete next[key]; return next; })}>Leave unresolved</Button>
          </div>
        </article>;
      })}
      {Object.entries(nextCursors).map(([batchId, cursor]) => <Button key={batchId} type="button" variant="outline" disabled={disabled} onClick={() => void loadMore(batchId, cursor)}>Load more matches · {batchId}</Button>)}
      {failure && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{failure}</p>}
      {acknowledgmentMissing && <p role="status" className="text-xs text-muted-foreground">A selected match has different recorded values. Check its acknowledgment before submitting, or leave it unresolved.</p>}
      {selectedCount > 200 && <p role="alert" className="text-sm">Select at most 200 decisions per confirmation.</p>}
      <Button type="button" disabled={disabled || selectedCount === 0 || selectedCount > 200 || acknowledgmentMissing} onClick={() => void submit()}>{submitting ? 'Saving decisions…' : `Submit ${selectedCount || ''} explicit decisions`}</Button>
    </section>
  );
}

function ValueList({ title, values, empty }: { title: string; values: Record<string, unknown>; empty?: string }) {
  const entries = Object.entries(values).filter(([key]) => key !== 'version');
  return <div className="space-y-1 text-xs">
    <p className="font-medium">{title}</p>
    {entries.length === 0 ? <p className="text-muted-foreground">{empty ?? 'No values'}</p> : <dl>
      {entries.map(([key, value]) => <div key={key} className="flex justify-between gap-3"><dt className="text-muted-foreground">{key}</dt><dd className="break-all text-right">{value === null || value === undefined ? '—' : typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : JSON.stringify(value)}</dd></div>)}
    </dl>}
  </div>;
}
function CandidateLabel({ candidate }: { candidate: T212ReviewCandidate }) {
  const attribution = candidate.import_source === 'trading212_api' ? 'Auto-imported' : candidate.import_source === 'statement_csv' ? 'CSV import' : candidate.import_source === 'legacy' ? 'Pre-feature or unknown import time' : 'Unknown provenance';
  return <span>{candidate.kind} {candidate.ledger_id} · {attribution}{candidate.values_differ ? ' · recorded values differ' : ' · exact recorded values'}</span>;
}
