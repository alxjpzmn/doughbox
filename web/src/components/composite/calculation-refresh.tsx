import { useEffect, useRef, useState } from 'react';
import useSwr, { useSWRConfig } from 'swr';
import { ArrowClockwiseIcon, CheckCircleIcon, CircleDashedIcon, SpinnerGapIcon, XCircleIcon } from '@phosphor-icons/react';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { formatDistanceToNowStrict } from 'date-fns';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Label } from '@/components/ui/label';

interface CalculationStatus {
  source: string;
  status: 'never_run' | 'running' | 'succeeded' | 'failed';
  started_at: string | null;
  finished_at: string | null;
  last_success_at: string | null;
  last_error: string | null;
  from_date: string | null;
  until_date: string | null;
  schedule_enabled: boolean;
  schedule_frequency: ScheduleFrequency;
  next_refresh_at: string | null;
}

type CalculationSource = 'performance' | 'taxation';
type ScheduleFrequency = 'daily' | 'weekly' | 'monthly';
interface ScheduleSettings {
  enabled: boolean;
  frequency: ScheduleFrequency;
}
const scheduleFrequencies: Record<ScheduleFrequency, string> = {
  daily: 'Daily',
  weekly: 'Weekly',
  monthly: 'Monthly',
};

const timestamp = (value: string | null) => {
  if (!value) return 'Never';
  const date = new Date(value);
  const exact = date.toLocaleString();
  const difference = date.getTime() - Date.now();
  const relative = Math.abs(difference) < 60_000
    ? difference > 0 ? 'In under a minute' : 'Just now'
    : formatDistanceToNowStrict(date, { addSuffix: true });
  return <time dateTime={value} title={exact} aria-label={`${relative} (${exact})`}>{relative}</time>;
};
const completionIdentity = (status: CalculationStatus) =>
  status.finished_at && (status.status === 'succeeded' || status.status === 'failed')
    ? `${status.source}/${status.started_at}/${status.finished_at}/${status.status}` : null;

const calculationState = (status: string) => {
  switch (status) {
    case 'running':
      return { label: 'Calculating', Icon: SpinnerGapIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' };
    case 'succeeded':
      return { label: 'Calculation succeeded', Icon: CheckCircleIcon, color: 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100' };
    case 'failed':
      return { label: 'Calculation failed', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' };
    default:
      return { label: 'Not run', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' };
  }
};

const CalculationScheduler = ({ source, status, stale, onSave }: {
  source: CalculationSource;
  status: CalculationStatus | undefined;
  stale: boolean;
  onSave: (settings: ScheduleSettings) => Promise<void>;
}) => {
  const [draft, setDraft] = useState<ScheduleSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const submitting = useRef(false);
  const disabled = stale || !status || saving;
  const name = source === 'performance' ? 'Performance' : 'Taxation';
  const save = async () => {
    if (!draft || disabled || submitting.current) return;
    submitting.current = true;
    setSaving(true);
    setFailure(null);
    try {
      await onSave(draft);
      setDraft(null);
    } catch (error: unknown) {
      setFailure(error && typeof error === 'object' && 'message' in error && typeof error.message === 'string'
        ? error.message : 'Check your connection and try again.');
    } finally {
      submitting.current = false;
      setSaving(false);
    }
  };

  if (!draft) return (
    <div aria-label={`${name} schedule`} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
      <div className="min-w-0 space-y-1">
        <p className="text-sm font-medium">{status
          ? status.schedule_enabled
            ? `${scheduleFrequencies[status.schedule_frequency]}${source === 'taxation' ? ' · All history' : ''}`
            : 'Schedule off'
          : 'Schedule unavailable'}</p>
        {status?.schedule_enabled && status.next_refresh_at && <p className="text-xs text-muted-foreground">
          Next run: {new Date(status.next_refresh_at).getTime() <= Date.now()
            ? <time dateTime={status.next_refresh_at} title={new Date(status.next_refresh_at).toLocaleString()}>Due now</time>
            : timestamp(status.next_refresh_at)}
        </p>}
      </div>
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => {
        if (!status) return;
        setDraft({ enabled: status.schedule_enabled, frequency: status.schedule_frequency });
        setFailure(null);
      }}>Manage scheduler</Button>
    </div>
  );

  return (
    <form aria-label={`${name} scheduler settings`} className="space-y-4 rounded-lg border p-3"
      onSubmit={event => { event.preventDefault(); void save(); }}>
      <div className="flex items-center gap-3">
        <Switch id={`${source}-calculation-schedule-enabled`} checked={draft.enabled} disabled={disabled}
          onCheckedChange={enabled => setDraft({ ...draft, enabled })} />
        <Label htmlFor={`${source}-calculation-schedule-enabled`}>Automatic refresh</Label>
      </div>
      <div className="max-w-56 space-y-2">
        <Label htmlFor={`${source}-calculation-schedule-frequency`}>Frequency</Label>
        <Select value={draft.frequency} disabled={disabled} onValueChange={frequency => {
          if (frequency === 'daily' || frequency === 'weekly' || frequency === 'monthly') setDraft({ ...draft, frequency });
        }}>
          <SelectTrigger id={`${source}-calculation-schedule-frequency`}><SelectValue>{scheduleFrequencies[draft.frequency]}</SelectValue></SelectTrigger>
          <SelectContent>
            {Object.entries(scheduleFrequencies).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {source === 'taxation' && <p className="text-xs text-muted-foreground">Scheduled runs refresh all history. Selected date ranges remain manual.</p>}
      <p className="text-xs text-muted-foreground">Enabling or changing a schedule makes it due within a minute. Monthly uses calendar months in UTC. Pausing does not cancel a running calculation.</p>
      {failure && <p role="alert" className="break-words text-xs text-red-700 dark:text-red-300">Could not save schedule: {failure}</p>}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={disabled}>{saving ? 'Saving schedule…' : 'Save schedule'}</Button>
        <Button type="button" variant="outline" size="sm" disabled={saving} onClick={() => {
          setDraft(null);
          setFailure(null);
        }}>Cancel</Button>
      </div>
    </form>
  );
};

export const CalculationRefresh = ({ source, fromDate, untilDate }: {
  source: CalculationSource;
  fromDate?: string;
  untilDate?: string;
}) => {
  const { mutate: mutateCache } = useSWRConfig();
  const statusUrl = `${BASE_URL}/${source}/status`;
  const { data: status, error, isLoading, mutate } = useSwr<CalculationStatus>(statusUrl, fetcher, {
    refreshInterval: current => current?.status === 'running' ? 2_000 : 60_000,
  });
  const submitting = useRef(false);
  const completion = useRef<string | null>(null);
  const [pending, setPending] = useState(false);
  const [, refreshRelativeDates] = useState(0);
  const [failure, setFailure] = useState<{ message: string; run: string } | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const run = status ? JSON.stringify([status.status, status.started_at, status.finished_at, status.last_error]) : '';
  const running = status?.status === 'running';
  const { label, Icon, color } = calculationState(pending ? 'running' : status?.status ?? 'never_run');
  const buttonLabel = source === 'performance' ? 'Refresh performance' : 'Recalculate taxation';
  const rangeDiffers = source === 'taxation' && status && status.status !== 'never_run'
    && ((status.from_date ?? undefined) !== fromDate || (status.until_date ?? undefined) !== untilDate);

  useEffect(() => {
    const timer = window.setInterval(() => refreshRelativeDates(value => value + 1), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!status) return;
    const identity = completionIdentity(status);
    if (!identity || completion.current === identity) return;
    // Observe the first completed status too: work may finish while this page is
    // unmounted, or the POST may already return a completed, very short job.
    completion.current = identity;
    setRefreshError(null);
    const endpoints = (source === 'performance'
      ? ['performance_overview', 'performance', 'past_performance', 'portfolio', 'positions']
      : ['taxation']).map(endpoint => `${BASE_URL}/${endpoint}`);
    void mutateCache(key => {
      if (Array.isArray(key)) return source === 'performance' && key[0] === 'positions';
      if (typeof key !== 'string') return false;
      const path = key.split('?')[0];
      if (path === statusUrl || path === `${BASE_URL}/${source}/refresh`) return false;
      return endpoints.some(endpoint => path === endpoint || path.startsWith(`${endpoint}/`));
    }).catch(() => {
      setRefreshError('The calculation finished, but saved results could not be reloaded. Reload this page to see the latest saved data.');
    });
  }, [status, source, statusUrl, mutateCache]);

  const startCalculation = async () => {
    if (submitting.current || !status || running || error) return;
    submitting.current = true;
    setPending(true);
    setFailure(null);
    const params = new URLSearchParams();
    if (source === 'taxation') {
      if (fromDate) params.set('from_date', fromDate);
      if (untilDate) params.set('until_date', untilDate);
    }
    const query = params.toString();
    try {
      const response = await sendMutateRequest(`${BASE_URL}/${source}/refresh${query ? `?${query}` : ''}`, undefined, { method: 'POST' });
      const updated: CalculationStatus = await response.json();
      await mutate(updated, { revalidate: false });
    } catch {
      setFailure({ message: 'Could not start the calculation. Check your connection and sign-in, then try again. If a job was accepted, its recorded status will appear automatically.', run });
    } finally {
      submitting.current = false;
      setPending(false);
      void mutate().catch(() => undefined);
    }
  };

  const saveSchedule = async (settings: ScheduleSettings) => {
    const response = await sendMutateRequest(`${BASE_URL}/${source}/scheduler`, settings, { method: 'PUT' });
    await mutate(await response.json() as CalculationStatus, { revalidate: false });
    void mutate().catch(() => undefined);
  };

  return (
    <Card className="mb-6" aria-label={`${source === 'performance' ? 'Performance' : 'Taxation'} calculation`}>
      <CardHeader>
        <CardTitle>{source === 'performance' ? 'Saved performance' : 'Saved taxation'}</CardTitle>
        <CardDescription>
          {source === 'performance'
            ? 'Update saved returns and portfolio history, including recorded income.'
            : 'Save a fresh tax report for the selected date range.'}
          {' '}Run manually or enable a schedule; navigation only reads saved results.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {isLoading && !status && <div role="status" aria-label="Loading calculation status"><Skeleton className="h-6 w-48" /></div>}
        {status && <>
          <span role="status" className="block" aria-label={`${source}: ${pending ? 'Starting calculation' : label}`}>
            <Badge variant="outline" className={color}>
              <Icon aria-hidden="true" className={running || pending ? 'motion-safe:animate-spin' : undefined} />
              {pending ? 'Starting calculation…' : label}
            </Badge>
          </span>
          <dl className={`grid grid-cols-2 gap-3 ${source === 'taxation' ? 'sm:grid-cols-3' : ''}`}>
            <div className="min-w-0 rounded-lg border bg-muted/20 p-3">
              <dt className="text-xs text-muted-foreground">Last success</dt>
              <dd className="mt-2 text-sm font-medium">{timestamp(status.last_success_at)}</dd>
            </div>
            {(status.started_at || pending) && <div className="min-w-0 rounded-lg border bg-muted/20 p-3">
              <dt className="text-xs text-muted-foreground">Started</dt>
              <dd className="mt-2 text-sm font-medium">{pending ? 'Starting…' : timestamp(status.started_at)}</dd>
              {!running && !pending && status.status !== 'succeeded' && status.finished_at && (
                <dd className="mt-1 text-xs text-muted-foreground">Finished {timestamp(status.finished_at)}</dd>
              )}
            </div>}
            {source === 'taxation' && <div className="col-span-2 min-w-0 rounded-lg border bg-muted/20 p-3 sm:col-span-1">
              <dt className="text-xs text-muted-foreground">Range</dt>
              <dd className="mt-2 break-words text-sm font-medium" title="Selected inclusive UTC date range">
                {!fromDate && !untilDate ? 'All history' : `${fromDate ?? 'Beginning'} → ${untilDate ?? 'Latest'}`}
              </dd>
              {rangeDiffers && <dd className="mt-1 break-words text-xs text-muted-foreground">
                {running ? 'Running' : 'Last run'}: {!status.from_date && !status.until_date
                  ? 'All history' : `${status.from_date ?? 'Beginning'} → ${status.until_date ?? 'Latest'}`}
              </dd>}
            </div>}
          </dl>
          {status.status === 'failed' && !pending && <details className="text-xs">
            <summary className="cursor-pointer font-medium text-red-700 dark:text-red-300">Last recorded error</summary>
            <p className="mt-2 break-words text-muted-foreground">{status.last_error ?? `The latest calculation failed. Try ${buttonLabel} again.`}</p>
          </details>}
        </>}
        {error && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">
          {status ? 'Calculation status could not be refreshed. Last loaded status is shown; the button is paused while automatic updates reconnect.' : 'Calculation status could not be loaded. The button will become available when automatic status updates reconnect.'}
        </p>}
        {failure?.run === run && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{failure.message}</p>}
        {refreshError && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{refreshError}</p>}
        <Button type="button" variant="outline" size="sm" disabled={!status || pending || running || !!error} onClick={() => void startCalculation()}>
          <ArrowClockwiseIcon aria-hidden="true" />
          {buttonLabel}
        </Button>
        {rangeDiffers && running && <p className="text-xs text-muted-foreground">A different range is running. Recalculate this selection after it finishes.</p>}
        <CalculationScheduler source={source} status={status} stale={!!error} onSave={saveSchedule} />
      </CardContent>
    </Card>
  );
};
