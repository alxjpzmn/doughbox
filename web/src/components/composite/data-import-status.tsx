import { useEffect, useRef, useState, type FormEvent } from 'react';
import useSwr, { useSWRConfig } from 'swr';
import { ArrowClockwiseIcon, CheckCircleIcon, CircleDashedIcon, XCircleIcon, SpinnerGapIcon } from '@phosphor-icons/react';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

interface DataImportStatus {
  source: string;
  available: boolean;
  unavailable_reason: string | null;
  status: string;
  started_at: string | null;
  finished_at: string | null;
  last_success_at: string | null;
  last_error: string | null;
  record_count: number;
  latest_record_date: string | null;
  schedule_enabled: boolean;
  schedule_frequency: ScheduleFrequency;
  next_refresh_at: string | null;
}

interface DataImportsResponse {
  sources: DataImportStatus[];
}

type ImportSource = 'polygon' | 'ecb';
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

const importsUrl = `${BASE_URL}/data-imports`;
const sources: { source: ImportSource; name: string; description: string }[] = [
  { source: 'polygon', name: 'Polygon stock splits', description: 'Fetch stock-split reference data for imported securities.' },
  { source: 'ecb', name: 'ECB exchange rates', description: 'Fetch European Central Bank exchange rates for currencies used by your portfolio.' },
];
const affectedEndpoints = [
  'instruments', 'prices', 'portfolio', 'positions', 'past_performance',
  'performance', 'performance_overview', 'timeline', 'taxation',
].map(endpoint => `${BASE_URL}/${endpoint}`);

const errorMessage = (error: unknown): string => {
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
    return error.message;
  }
  return 'A network error occurred. Please try again.';
};

const timestamp = (value: string | null) => value ? new Date(value).toLocaleString() : 'Not yet';
const completionIdentity = (status: DataImportStatus) =>
  status.finished_at && (status.status === 'succeeded' || status.status === 'failed')
    ? `${status.started_at}/${status.finished_at}/${status.status}` : null;

const importState = (status: string) => {
  switch (status) {
    case 'running':
      return { label: 'Importing', Icon: SpinnerGapIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' };
    case 'succeeded':
      return { label: 'Import succeeded', Icon: CheckCircleIcon, color: 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100' };
    case 'failed':
      return { label: 'Import failed', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' };
    default:
      return { label: 'Not run', Icon: CircleDashedIcon, color: 'bg-muted text-muted-foreground' };
  }
};

const SchedulerControls = ({ source, name, status, stale, onSave }: {
  source: ImportSource;
  name: string;
  status: DataImportStatus | undefined;
  stale: boolean;
  onSave: (source: ImportSource, settings: ScheduleSettings) => Promise<void>;
}) => {
  const [draft, setDraft] = useState<ScheduleSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const submitting = useRef(false);
  const disabled = stale || !status || saving;

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!draft || disabled || submitting.current) return;
    submitting.current = true;
    setSaving(true);
    setFailure(null);
    try {
      await onSave(source, draft);
      setDraft(null);
    } catch (error: unknown) {
      setFailure(errorMessage(error));
    } finally {
      submitting.current = false;
      setSaving(false);
    }
  };

  if (!draft) return (
    <div className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
      <p className="text-xs text-muted-foreground">
        {status ? status.schedule_enabled ? `Automatically scheduled (${status.schedule_frequency})` : 'Manual refresh only' : 'Schedule unavailable'}
      </p>
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => {
        if (!status) return;
        setDraft({ enabled: status.schedule_enabled, frequency: status.schedule_frequency });
        setFailure(null);
      }}>Manage scheduler</Button>
    </div>
  );

  return (
    <form onSubmit={save} aria-label={`${name} scheduler settings`} className="mt-3 space-y-4 rounded-lg border p-3">
      <div className="flex items-center gap-3">
        <Switch id={`${source}-scheduler-enabled`} checked={draft.enabled} disabled={disabled}
          onCheckedChange={enabled => setDraft({ ...draft, enabled })} />
        <Label htmlFor={`${source}-scheduler-enabled`}>Automatic refresh</Label>
      </div>
      <div className="max-w-56 space-y-2">
        <Label htmlFor={`${source}-scheduler-frequency`}>Frequency</Label>
        <Select value={draft.frequency} disabled={disabled} onValueChange={frequency => {
          if (frequency === 'daily' || frequency === 'weekly' || frequency === 'monthly') setDraft({ ...draft, frequency });
        }}>
          <SelectTrigger id={`${source}-scheduler-frequency`}><SelectValue>{scheduleFrequencies[draft.frequency]}</SelectValue></SelectTrigger>
          <SelectContent>
            {Object.entries(scheduleFrequencies).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <p className="text-xs text-muted-foreground">
        Settings persist across restarts. Enabling or changing an enabled schedule makes it due on the next scheduler check, within a minute.
        Monthly schedules use calendar months in UTC. Pausing does not cancel a running import.
      </p>
      {failure && <p role="alert" className="break-words text-xs text-red-700 dark:text-red-300">Could not save schedule: {failure}</p>}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={disabled}>{saving ? 'Saving schedule…' : 'Save schedule'}</Button>
        <Button type="button" size="sm" variant="outline" disabled={saving} onClick={() => {
          setDraft(null);
          setFailure(null);
        }}>Cancel</Button>
      </div>
    </form>
  );
};

const ImportSourceCard = ({ source, name, description, status, stale, onImport, onSaveSchedule }: {
  source: ImportSource;
  name: string;
  description: string;
  status: DataImportStatus | undefined;
  stale: boolean;
  onImport: (source: ImportSource) => Promise<void>;
  onSaveSchedule: (source: ImportSource, settings: ScheduleSettings) => Promise<void>;
}) => {
  const submitting = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ message: string; run: string } | null>(null);
  const run = status ? JSON.stringify([
    status.available, status.status, status.started_at, status.finished_at, status.last_error,
  ]) : '';
  const { label, Icon, color } = importState(pending ? 'running' : status?.status ?? 'never_run');
  const running = status?.status === 'running';
  const active = running || status?.status === 'queued';

  const startImport = async () => {
    if (submitting.current || !status?.available || active || stale) return;
    submitting.current = true;
    setPending(true);
    setFailure(null);
    try {
      await onImport(source);
    } catch (error: unknown) {
      setFailure({ message: errorMessage(error), run });
    } finally {
      submitting.current = false;
      setPending(false);
    }
  };

  return (
    <article className="min-w-0 rounded-lg border bg-muted/20 p-3" aria-labelledby={`${source}-import-title`}>
      <h3 id={`${source}-import-title`} className="font-semibold">{name}</h3>
      <p className="mb-3 text-xs text-muted-foreground">{description}</p>
      {status && <span role="status" aria-label={`${name}: ${pending ? 'Starting import' : label}`}>
        <Badge variant="outline" className={color}>
          <Icon aria-hidden="true" className={running || pending ? 'motion-safe:animate-spin' : undefined} />
          {pending ? 'Starting import…' : label}
        </Badge>
      </span>}
      {status && <>
          <dl className="mt-3 space-y-3 text-xs">
            {status.schedule_enabled && <div>
              <dt className="text-muted-foreground">Next scheduled import</dt>
              <dd className="mt-1 break-words">{status.next_refresh_at ? timestamp(status.next_refresh_at) : 'Awaiting scheduler'}</dd>
            </div>}
            <div>
              <dt className="text-muted-foreground">Last successful import</dt>
              <dd className="mt-1 break-words">{timestamp(status.last_success_at)}</dd>
            </div>
            {status.started_at && <div>
              <dt className="text-muted-foreground">Latest import started</dt>
              <dd className="mt-1 break-words">{timestamp(status.started_at)}</dd>
            </div>}
            {!running && !pending && status.finished_at && <div>
              <dt className="text-muted-foreground">Latest import finished</dt>
              <dd className="mt-1 break-words">{timestamp(status.finished_at)}</dd>
            </div>}
            <div>
              <dt className="text-muted-foreground">Stored {source === 'polygon' ? 'stock-split' : 'exchange-rate'} rows</dt>
              <dd className="mt-1 tabular-nums">{status.record_count.toLocaleString()}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Latest stored record</dt>
              <dd className="mt-1">{status.latest_record_date ?? 'No stored records'}</dd>
            </div>
          </dl>
          {!status.available && <p role="note" className="mt-3 break-words text-xs text-muted-foreground">
            {status.unavailable_reason ?? 'This import is unavailable. Check the server configuration; status updates automatically.'}
            {' '}You can configure its schedule, but imports will not run until the provider is configured and available.
          </p>}
          {status.status === 'failed' && !pending && <details className="mt-3 text-xs">
            <summary className="cursor-pointer font-medium text-red-700 dark:text-red-300">Last recorded error</summary>
            <p className="mt-2 break-words text-muted-foreground">
              {status.last_error ?? 'The latest import failed. Try Refresh now again.'}
            </p>
          </details>}
      </>}
      {!status && <p role="alert" className="mt-3 text-xs text-red-700 dark:text-red-300">This source’s status is missing. Controls will become available when automatic status updates load it.</p>}
      {failure?.run === run && <p role="alert" className="mt-3 break-words text-xs text-red-700 dark:text-red-300">Could not start import: {failure.message}</p>}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm"
          disabled={!status?.available || pending || active || stale}
          aria-label={`Refresh ${name} now`}
          onClick={() => void startImport()}>
          <ArrowClockwiseIcon aria-hidden="true" />
          Refresh now
        </Button>
      </div>
      <SchedulerControls source={source} name={name} status={status} stale={stale} onSave={onSaveSchedule} />
    </article>
  );
};

export const DataImportStatusPanel = () => {
  const { mutate: mutateCache } = useSWRConfig();
  const { data, error, isLoading, mutate } = useSwr<DataImportsResponse>(importsUrl, fetcher, {
    refreshInterval: current => current?.sources.some(source => source.status === 'running') ? 2_000 : 60_000,
  });
  const completions = useRef<Map<string, string | null> | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);

  useEffect(() => {
    if (!data) return;
    if (!completions.current) {
      completions.current = new Map(data.sources.map(source => [source.source, completionIdentity(source)]));
      return;
    }
    let finished = false;
    for (const source of data.sources) {
      const completion = completionIdentity(source);
      if (completion && completion !== completions.current.get(source.source)) finished = true;
      if (completion) completions.current.set(source.source, completion);
    }
    if (!finished) return;
    setRefreshError(null);
    void mutateCache(key => {
      // Historical holdings use a tuple key rather than an API URL.
      if (Array.isArray(key)) return key[0] === 'positions';
      if (typeof key !== 'string') return false;
      const path = key.split('?')[0];
      return affectedEndpoints.some(endpoint => path === endpoint || path.startsWith(`${endpoint}/`));
    }).catch((failure: unknown) => {
      setRefreshError(`The import finished, but related portfolio data could not be refreshed. Reload to see current calculations. ${errorMessage(failure)}`);
    });
  }, [data, mutateCache]);

  const startImport = async (source: ImportSource) => {
    try {
      const response = await sendMutateRequest(`${importsUrl}/${source}/refresh`, undefined, { method: 'POST' });
      const updated: DataImportsResponse = await response.json();
      // Concurrent source requests must not overwrite each other's newer status.
      await mutate(current => current ? {
        sources: current.sources.map(item => item.source === source
          ? updated.sources.find(candidate => candidate.source === source) ?? item : item),
      } : updated, { revalidate: false });
    } finally {
      // Also refresh availability after a rejected request (for example, a missing token).
      void mutate().catch(() => undefined);
    }
  };

  const saveSchedule = async (source: ImportSource, settings: ScheduleSettings) => {
    const response = await sendMutateRequest(`${importsUrl}/${source}/scheduler`, settings, { method: 'PUT' });
    const updated: DataImportsResponse = await response.json();
    await mutate(current => current ? {
      sources: current.sources.map(item => item.source === source
        ? updated.sources.find(candidate => candidate.source === source) ?? item : item),
    } : updated, { revalidate: false });
  };

  return (
    <Card aria-label="Reference data imports">
      <CardHeader>
        <CardTitle>Reference data</CardTitle>
        <CardDescription>Schedule or manually refresh stock splits and exchange rates for your portfolio.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="flex items-center gap-2 text-xs text-muted-foreground">
          <ArrowClockwiseIcon aria-hidden="true" />
          Status updates automatically · every minute, or every 2 seconds during imports
        </p>
        {error && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{data ? 'Import status could not be refreshed. Last loaded results are shown; refresh and schedule controls are paused while automatic updates reconnect. ' : 'Import status could not be loaded. Retrying automatically. '}{errorMessage(error)}</p>}
        {refreshError && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{refreshError}</p>}
        {isLoading && !data && <div role="status" aria-label="Loading reference data import status" className="grid gap-3 sm:grid-cols-2">
          {sources.map(source => <div key={source.source} className="min-w-0 space-y-3 rounded-lg border bg-muted/20 p-3">
            <Skeleton className="h-5 w-2/3" /><Skeleton className="h-4 w-full" /><Skeleton className="h-8 w-1/2" />
          </div>)}
        </div>}
        {data && <div aria-label="Reference data sources" className="grid gap-3 sm:grid-cols-2">
          {sources.map(source => <ImportSourceCard key={source.source} {...source}
            status={data.sources.find(status => status.source === source.source)} stale={!!error} onImport={startImport} onSaveSchedule={saveSchedule} />)}
        </div>}
        <p className="text-xs text-muted-foreground">
          Status persists across restarts and tracks scheduled imports and imports started here or through the API, not CLI housekeeping or implicit exchange-rate fetches.
          Stored counts include all imports. Refreshing reference data does not change price-feed schedules.
        </p>
      </CardContent>
    </Card>
  );
};
