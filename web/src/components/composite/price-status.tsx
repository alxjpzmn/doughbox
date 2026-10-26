import { useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import { ArrowClockwiseIcon, CheckCircleIcon, CircleDashedIcon, XCircleIcon, ClockIcon, SpinnerGapIcon } from '@phosphor-icons/react';
import { PriceFeedStatus, PriceSchedulerSettings, PricingStatus } from '@/types/core';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { formatCurrency } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

const timestamp = (value?: Date | null) => value ? new Date(String(value)).toLocaleString() : 'Not yet';

const marketSources: Record<string, string> = {
  deutsche_boerse: 'Deutsche Börse',
  cboe: 'Cboe Europe',
  euronext: 'Euronext',
};

const sourceLabels: Record<string, string> = {
  ...marketSources,
  execution_fallback: 'Last imported execution',
  legacy_unknown: 'Existing stored price',
  manual: 'Manual override',
  missing: 'Missing',
  unavailable: 'Unavailable',
};

const exchangeNames: Record<string, string> = {
  'DETR-posttrade': 'Xetra',
  'DGAT-posttrade': 'Tradegate',
  'DFRA-posttrade': 'Frankfurt',
  'CBOE-posttrade': 'Cboe Europe',
  'EURONEXT-posttrade': 'Euronext',
};

const importState = (feed: PriceFeedStatus) => {
  switch (feed.job_status) {
    case 'succeeded':
      return { label: 'Import succeeded', Icon: CheckCircleIcon, color: 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100' };
    case 'failed':
      return { label: 'Import failed', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' };
    case 'running':
      return { label: 'Importing', Icon: SpinnerGapIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' };
    case 'queued':
      return { label: feed.last_error && feed.attempts > 0 ? 'Retry scheduled' : 'Queued', Icon: ClockIcon, color: 'border-amber-200 bg-amber-100 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100' };
    default:
      return { label: feed.job_status ?? 'Not run', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' };
  }
};

const SchedulerControls = ({ data, disabled, onSave }: {
  data: PricingStatus;
  disabled: boolean;
  onSave: (settings: PriceSchedulerSettings) => Promise<boolean>;
}) => {
  const [draft, setDraft] = useState<PriceSchedulerSettings | null>(null);
  const [intervalMinutes, setIntervalMinutes] = useState('');
  const [workerConcurrency, setWorkerConcurrency] = useState('');
  const selected = data.feeds.filter(feed => feed.enabled);

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!draft) return;
    if (await onSave({ ...draft, interval_minutes: Number(intervalMinutes), worker_concurrency: Number(workerConcurrency) })) setDraft(null);
  };

  if (!draft) return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
      <p className="text-sm text-muted-foreground">{selected.length} of {data.feeds.length} exchanges selected · worker limit {data.worker_concurrency} per process</p>
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => {
        setDraft({ enabled: data.enabled, interval_minutes: data.interval_minutes, worker_concurrency: data.worker_concurrency, feeds: selected.map(feed => feed.feed) });
        setIntervalMinutes(String(data.interval_minutes));
        setWorkerConcurrency(String(data.worker_concurrency));
      }}>Manage scheduler</Button>
    </div>
  );

  return (
    <form onSubmit={save} aria-label="Price scheduler settings" className="space-y-4 rounded-lg border p-4">
      <div className="flex items-center gap-3">
        <Switch id="price-scheduler-enabled" checked={draft.enabled} disabled={disabled} onCheckedChange={enabled => setDraft({ ...draft, enabled })} />
        <Label htmlFor="price-scheduler-enabled">Automatic refresh</Label>
      </div>
      <div className="max-w-56 space-y-2">
        <Label htmlFor="price-scheduler-interval">Interval (minutes)</Label>
        <Input id="price-scheduler-interval" type="number" min={1} max={1440} step={1} required disabled={disabled}
          value={intervalMinutes} onChange={event => setIntervalMinutes(event.target.value)} />
      </div>
      <div className="max-w-56 space-y-2">
        <Label htmlFor="price-worker-concurrency">Concurrent import workers</Label>
        <Input id="price-worker-concurrency" type="number" min={1} max={4} step={1} required disabled={disabled}
          value={workerConcurrency} onChange={event => setWorkerConcurrency(event.target.value)} />
      </div>
      <fieldset disabled={disabled} className="space-y-3">
        <legend className="mb-2 text-sm font-medium">Automatically refreshed exchanges</legend>
        <div className="grid gap-3 sm:grid-cols-2">
          {data.feeds.map(feed => <div key={feed.feed} className="flex items-center gap-3">
            <Switch id={`scheduled-${feed.feed}`} checked={draft.feeds.includes(feed.feed)} disabled={disabled} onCheckedChange={checked => {
              setDraft(current => current && ({
                ...current,
                feeds: checked ? [...current.feeds, feed.feed] : current.feeds.filter(key => key !== feed.feed),
              }));
            }} />
            <Label htmlFor={`scheduled-${feed.feed}`}>{exchangeNames[feed.feed] ?? feed.feed}</Label>
          </div>)}
        </div>
      </fieldset>
      <p className="text-xs text-muted-foreground">
        Settings persist across restarts and apply without restarting the server.
        Enabling queues selected exchanges immediately. Pausing leaves queued and running imports unchanged.
        Worker limits apply per process within two seconds. Lowering the limit lets active imports finish before new work starts.
      </p>
      {draft.enabled && draft.feeds.length === 0 && <p role="alert" className="text-sm text-red-700 dark:text-red-300">Select at least one exchange to enable automatic refresh.</p>}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={disabled || (draft.enabled && draft.feeds.length === 0)}>Save schedule</Button>
        <Button type="button" size="sm" variant="outline" disabled={disabled} onClick={() => setDraft(null)}>Cancel</Button>
      </div>
    </form>
  );
};

const ExchangeImportStatus = ({ feed, disabled, onAction }: {
  feed: PriceFeedStatus;
  disabled: boolean;
  onAction: (feed: string, action: 'refresh' | 'reset') => Promise<boolean>;
}) => {
  const name = exchangeNames[feed.feed] ?? feed.feed;
  const { label, Icon, color } = importState(feed);
  const [confirmReset, setConfirmReset] = useState(false);

  return (
    <article aria-label={`${name} import status`} className="min-w-0 rounded-lg border bg-muted/20 p-3">
      <h3 className="font-semibold">{name}</h3>
      <p className="mb-3 break-all font-mono text-xs text-muted-foreground">{feed.feed}</p>
      <p className="mb-3 text-xs text-muted-foreground">{feed.scheduled ? 'Automatically scheduled' : feed.enabled ? 'Selected · scheduler paused' : 'Manual refresh only'}</p>
      <Badge variant="outline" className={color}>
        <Icon aria-hidden="true" className={feed.job_status === 'running' ? 'motion-safe:animate-spin' : undefined} />
        {label}
      </Badge>
      <dl className="mt-3 space-y-3 text-xs">
        <div>
          <dt className="text-muted-foreground">Last successful import</dt>
          <dd className="mt-1 break-words">{timestamp(feed.last_success_at)}</dd>
        </div>
        {feed.job_status && <div>
          <dt className="text-muted-foreground">Latest job{feed.attempts > 0 && ` · attempt ${feed.attempts}`}</dt>
          <dd className="mt-1">{feed.files_processed.toLocaleString()} batches · {feed.reports_processed.toLocaleString()} reports</dd>
        </div>}
        {feed.scheduled && <div>
          <dt className="text-muted-foreground">Next scheduled import</dt>
          <dd className="mt-1 break-words">{timestamp(feed.next_refresh_at)}</dd>
        </div>}
      </dl>
      {feed.last_error && <details className="mt-3 text-xs">
        <summary className="cursor-pointer font-medium text-red-700 dark:text-red-300">Last recorded error</summary>
        <p className="mt-2 break-words text-muted-foreground">{feed.last_error}</p>
      </details>}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" disabled={disabled} aria-label={`Refresh ${name} now`}
          onClick={() => onAction(feed.feed, 'refresh')}><ArrowClockwiseIcon aria-hidden="true" />Refresh now</Button>
        <Button type="button" variant="outline" size="sm" disabled={disabled || confirmReset}
          aria-label={`Reset ${name} import`} onClick={() => setConfirmReset(true)}>Reset import</Button>
      </div>
      {confirmReset && <div role="group" aria-label={`${name} reset confirmation`} className="mt-3 space-y-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-xs dark:border-amber-800 dark:bg-amber-950">
        <p>Replace the current {name} run with a fresh attempt? Completed checkpoints, stored prices, manual pins and job history are kept. A queued or running job will be retired.</p>
        <div className="flex flex-wrap gap-2">
          <Button type="button" size="sm" variant="destructive" disabled={disabled} aria-label={`Confirm reset for ${name}`} onClick={async () => {
            if (await onAction(feed.feed, 'reset')) setConfirmReset(false);
          }}>Confirm reset</Button>
          <Button type="button" size="sm" variant="outline" disabled={disabled} onClick={() => setConfirmReset(false)}>Cancel reset</Button>
        </div>
      </div>}
    </article>
  );
};

export const PriceStatus = () => {
  const { data, error, mutate } = useSwr<PricingStatus>(`${BASE_URL}/prices`, fetcher, {
    refreshInterval: status => status?.feeds.some(feed => feed.job_status === 'queued' || feed.job_status === 'running') ? 2_000 : 60_000,
  });
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ error: boolean; message: string } | null>(null);
  const submitting = useRef(false);

  const submit = async (url: string, method: 'POST' | 'PUT', message: string, settings?: PriceSchedulerSettings): Promise<boolean> => {
    if (submitting.current) return false;
    submitting.current = true;
    setBusy(true);
    setFeedback(null);
    try {
      const response = await sendMutateRequest(url, settings, { method });
      const updated: PricingStatus = await response.json();
      await mutate(updated, { revalidate: false });
      setFeedback({ error: false, message });
      return true;
    } catch (failure) {
      const message = failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
        ? failure.message : 'Price controls could not be updated. Refresh the status before retrying.';
      setFeedback({ error: true, message });
      return false;
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };

  if (!data && !error) return null;

  const automatic = data?.quotes.filter(quote => marketSources[quote.source] != null && quote.price != null).length ?? 0;
  const missing = data?.quotes.filter(quote => quote.price == null).length ?? 0;
  const older = data?.quotes.filter(quote => quote.older_than_hour).length ?? 0;

  return (
    <Card>
      <CardHeader>
        <CardTitle>Instrument prices</CardTitle>
        <CardDescription>
          {error ? (data ? 'Price status could not be refreshed. Last loaded results are shown.' : 'Price refresh status could not be loaded.') : data?.enabled
            ? `Automatic refresh every ${data.interval_minutes} minutes`
            : 'Automatic refresh is paused. Manage the schedule below or refresh an exchange manually.'}
        </CardDescription>
      </CardHeader>
      {data && <CardContent className="space-y-3">
        <SchedulerControls data={data} disabled={busy || !!error} onSave={settings =>
          submit(`${BASE_URL}/prices/scheduler`, 'PUT', 'Scheduler settings saved.', settings)} />
        <Button type="button" variant="outline" size="sm" disabled={busy || !!error || !data.feeds.some(feed => feed.enabled)}
          onClick={() => submit(`${BASE_URL}/prices/refresh`, 'POST', 'Refresh queued for selected exchanges. Existing work and retry backoff are preserved.')}>
          <ArrowClockwiseIcon aria-hidden="true" />Refresh selected exchanges now
        </Button>
        {busy && <p role="status" className="text-sm text-muted-foreground">Updating price controls…</p>}
        {feedback && <p role={feedback.error ? 'alert' : 'status'} className={`text-sm ${feedback.error ? 'text-red-700 dark:text-red-300' : 'text-muted-foreground'}`}>{feedback.message}</p>}
        <p className="text-sm text-muted-foreground">
          {automatic} of {data.quotes.length} holdings use delayed market prices
          {older > 0 && ` · ${older} prices older than one hour`}
          {missing > 0 && ` · ${missing} missing prices`}
        </p>
        <div aria-label="Exchange imports" className="grid gap-3 sm:grid-cols-3">
          {data.feeds.map(feed => <ExchangeImportStatus key={feed.feed} feed={feed} disabled={busy || !!error} onAction={(key, action) =>
            submit(`${BASE_URL}/prices/feeds/${encodeURIComponent(key)}/${action}`, 'POST',
              action === 'reset' ? `${exchangeNames[key] ?? key}: fresh attempt queued; completed checkpoints and prices are kept.` : `${exchangeNames[key] ?? key}: refresh queued.`)} />)}
        </div>
        <p className="text-xs text-muted-foreground">A successful import means the refresh completed; price coverage still varies by instrument.</p>
        <details>
          <summary className="cursor-pointer text-sm">Holding price sources and timestamps</summary>
          <Table>
            <TableHeader><TableRow>
              <TableHead>ISIN</TableHead>
              <TableHead>EUR price</TableHead>
              <TableHead>Source</TableHead>
              <TableHead>Trade / valuation time</TableHead>
            </TableRow></TableHeader>
            <TableBody>
              {data.quotes.map(quote => <TableRow key={quote.isin}>
                <TableCell>{quote.isin}</TableCell>
                <TableCell>{quote.price == null ? 'Unavailable' : formatCurrency(parseFloat(quote.price))}</TableCell>
                <TableCell>
                  {sourceLabels[quote.source] ?? quote.source}{quote.venue && ` · ${quote.venue}`}{quote.pinned && ' · Pinned'}
                </TableCell>
                <TableCell>{timestamp(quote.as_of)}{quote.older_than_hour && ' · Older than 1h'}</TableCell>
              </TableRow>)}
            </TableBody>
          </Table>
        </details>
        <p className="text-xs text-muted-foreground">
          Market reports are delayed; a 30-minute refresh can leave prices approximately 45 minutes behind the market.
          Price age includes nights, weekends and market holidays. These are not live quotes or official closing prices.
        </p>
      </CardContent>}
    </Card>
  );
};
