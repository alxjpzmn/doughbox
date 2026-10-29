import { useEffect, useRef, useState, type FormEvent } from 'react';
import useSWR, { useSWRConfig } from 'swr';
import { CheckCircleIcon, ClockIcon, SpinnerGapIcon, UploadSimpleIcon, XCircleIcon } from '@phosphor-icons/react';
import { BASE_URL, fetcher } from '@/lib/http';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

interface StatementImportJob {
  id: string;
  filename: string;
  size_bytes: number;
  status: 'queued' | 'running' | 'succeeded' | 'failed';
  queued_at: string;
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
}

interface StatementImportsResponse {
  jobs: StatementImportJob[];
}

const importsUrl = `${BASE_URL}/statement-imports`;
const maxFileBytes = 25 * 1024 * 1024;
const maxBatchBytes = 100 * 1024 * 1024;
const affectedEndpoints = [
  'timeline', 'instruments', 'brokers', 'prices', 'portfolio', 'positions', 'trades',
  'buy-ins', 'dividends', 'interest', 'fx-conversions', 'performance',
  'performance_overview', 'past_performance', 'taxation', 'data-imports',
].map(endpoint => `${BASE_URL}/${endpoint}`);
const errorMessage = (error: unknown) => error && typeof error === 'object' && 'message' in error
  && typeof error.message === 'string' ? error.message : 'A network error occurred. Please try again.';
const states = {
  queued: { label: 'Queued', Icon: ClockIcon, color: 'border-amber-200 bg-amber-100 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100' },
  running: { label: 'Importing', Icon: SpinnerGapIcon, color: 'border-blue-200 bg-blue-100 text-blue-800 dark:border-blue-800 dark:bg-blue-900 dark:text-blue-100' },
  succeeded: { label: 'Succeeded', Icon: CheckCircleIcon, color: 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100' },
  failed: { label: 'Failed', Icon: XCircleIcon, color: 'border-red-200 bg-red-100 text-red-800 dark:border-red-800 dark:bg-red-900 dark:text-red-100' },
};

export const StatementImportsPanel = () => {
  const { mutate: mutateCache } = useSWRConfig();
  const { data, error, isLoading, mutate } = useSWR<StatementImportsResponse>(importsUrl, fetcher, {
    refreshInterval: current => current?.jobs.some(job => job.status === 'queued' || job.status === 'running') ? 2_000 : 60_000,
  });
  const input = useRef<HTMLInputElement>(null);
  const submitting = useRef(false);
  const completions = useRef<Map<string, string | null> | null>(null);
  const [files, setFiles] = useState<File[]>([]);
  const [uploading, setUploading] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);

  useEffect(() => {
    if (!data) return;
    if (!completions.current) {
      completions.current = new Map(data.jobs.map(job => [job.id, job.finished_at]));
      return;
    }
    let finished = false;
    for (const job of data.jobs) {
      if (job.finished_at && completions.current.get(job.id) !== job.finished_at) finished = true;
    }
    completions.current = new Map(data.jobs.map(job => [job.id, job.finished_at]));
    if (!finished) return;
    // Failed imports can have written records before the error, so refresh those too.
    setRefreshError(null);
    void mutateCache(key => {
      if (Array.isArray(key)) return key[0] === 'positions';
      if (typeof key !== 'string') return false;
      const path = key.split('?')[0];
      return affectedEndpoints.some(endpoint => path === endpoint || path.startsWith(`${endpoint}/`));
    }).catch((error: unknown) => setRefreshError(`Import status is current, but portfolio data could not be refreshed. Reload to see current data. ${errorMessage(error)}`));
  }, [data, mutateCache]);

  const upload = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (submitting.current || !files.length || !data || error) return;
    setFailure(null);
    setNotice(null);
    if (files.length > 100) {
      setFailure('Choose at most 100 files per upload.');
      return;
    }
    const invalid = files.find(file => !/\.(pdf|csv)$/i.test(file.name) || !file.size || file.size > maxFileBytes);
    if (invalid) {
      setFailure(`${invalid.name}: choose a non-empty PDF or CSV file up to 25 MiB.`);
      return;
    }
    // Leave space for multipart headers within the server's request-body limit.
    if (files.reduce((total, file) => total + file.size, 0) > maxBatchBytes - 1024 * 1024) {
      setFailure('This upload is too large. Keep the combined file size below 99 MiB or split it into smaller batches.');
      return;
    }
    submitting.current = true;
    setUploading(true);
    try {
      const body = new FormData();
      for (const file of files) body.append('files', file);
      const accepted: StatementImportsResponse = await fetcher(importsUrl, { method: 'POST', body });
      setFiles([]);
      if (input.current) input.current.value = '';
      setNotice(`${accepted.jobs.length} ${accepted.jobs.length === 1 ? 'file' : 'files'} added to the import queue.`);
      // Do not overwrite a more recent poll with the queued state from the upload response.
      await mutate(current => ({ jobs: [
        ...accepted.jobs.filter(job => !current?.jobs.some(existing => existing.id === job.id)),
        ...current?.jobs ?? [],
      ] }), { revalidate: false });
    } catch (error: unknown) {
      setFailure(`Upload could not be confirmed: ${errorMessage(error)} Check the queue before submitting again.`);
    } finally {
      submitting.current = false;
      setUploading(false);
      void mutate().catch(() => undefined);
    }
  };

  const queued = data?.jobs.filter(job => job.status === 'queued').length ?? 0;
  const running = data?.jobs.filter(job => job.status === 'running').length ?? 0;

  return (
    <div className="mb-6">
      <Button type="button" variant="outline" aria-expanded={expanded} aria-controls="statement-import-section"
        onClick={() => setExpanded(!expanded)}>
        <UploadSimpleIcon aria-hidden="true" />
        {expanded ? 'Hide imports' : 'Import'}
      </Button>
      <Card id="statement-import-section" hidden={!expanded} className="mt-4" aria-label="Brokerage statement imports">
      <CardHeader>
        <CardTitle>Import statements</CardTitle>
        <CardDescription>Upload PDF or CSV brokerage statements. Imports run one file at a time, even after you leave this tab.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form onSubmit={upload} className="space-y-3" aria-label="Upload brokerage statements">
          <div className="space-y-2">
            <Label htmlFor="statement-import-files">Statement files</Label>
            <div role="group" aria-label="Drop statement files" aria-disabled={uploading}
              className={`rounded-lg border-2 border-dashed p-6 text-center transition-colors ${dragging ? 'border-primary bg-primary/10' : 'border-border bg-muted/20'} ${uploading ? 'opacity-50' : ''}`}
              onDragEnter={event => {
                event.preventDefault();
                if (uploading || !event.dataTransfer.types.includes('Files')) return;
                dragDepth.current += 1;
                setDragging(true);
              }}
              onDragOver={event => {
                event.preventDefault();
                event.dataTransfer.dropEffect = uploading ? 'none' : 'copy';
              }}
              onDragLeave={event => {
                event.preventDefault();
                dragDepth.current = Math.max(0, dragDepth.current - 1);
                if (dragDepth.current === 0) setDragging(false);
              }}
              onDrop={event => {
                event.preventDefault();
                dragDepth.current = 0;
                setDragging(false);
                if (uploading || event.dataTransfer.files.length === 0) return;
                setFiles(Array.from(event.dataTransfer.files));
                if (input.current) input.current.value = '';
                setFailure(null);
                setNotice(null);
              }}>
              <UploadSimpleIcon aria-hidden="true" size={28} className="mx-auto mb-2 text-muted-foreground" />
              <p className="mb-3 text-sm">{dragging ? 'Drop files here' : 'Drag and drop PDF or CSV files here'}</p>
              <Button type="button" variant="outline" disabled={uploading} aria-describedby="statement-import-limits"
                onClick={() => input.current?.click()}>
                Choose files
              </Button>
              <Input ref={input} id="statement-import-files" type="file" multiple accept=".pdf,.csv" disabled={uploading}
                className="hidden" tabIndex={-1} aria-describedby="statement-import-limits" onChange={event => {
                  setFiles(Array.from(event.target.files ?? []));
                  setFailure(null);
                  setNotice(null);
                }} />
            </div>
            <p id="statement-import-limits" className="text-xs text-muted-foreground">Up to 100 files, 25 MiB per file, 99 MiB combined per upload. Supported brokers are detected automatically.</p>
          </div>
          {files.length > 0 && <ul aria-label="Selected statement files" className="max-h-32 overflow-y-auto text-xs text-muted-foreground">
            {files.map((file, index) => <li className="break-all" key={index}>{file.name} · {(file.size / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} KiB</li>)}
          </ul>}
          <Button type="submit" disabled={uploading || !files.length || !data || !!error}>
            {uploading ? <SpinnerGapIcon aria-hidden="true" className="animate-spin" /> : <UploadSimpleIcon aria-hidden="true" />}
            {uploading ? 'Uploading…' : 'Upload and queue'}
          </Button>
          {uploading && <p role="status" className="text-xs text-muted-foreground">Keep this page open until the upload is confirmed. Imports then continue on the server.</p>}
          {failure && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{failure}</p>}
          {notice && <p role="status" className="text-sm">{notice}</p>}
        </form>
        {error && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{data ? 'Import status could not be refreshed. Last loaded results are shown; uploads are paused until status reconnects. ' : 'Import status could not be loaded. Retrying automatically. '}{errorMessage(error)}</p>}
        {refreshError && <p role="alert" className="break-words text-sm text-red-700 dark:text-red-300">{refreshError}</p>}
        {isLoading && !data && <p role="status" className="text-sm text-muted-foreground">Loading import queue…</p>}
        {data && <>
          <p role="status" aria-live="polite" className="text-sm">{running} importing · {queued} queued</p>
          <p className="text-xs text-muted-foreground">Status updates automatically · every 2 seconds during imports, every minute while idle. Showing all active files and the latest 100 results.</p>
          {data.jobs.length === 0 ? <p className="text-sm text-muted-foreground">No statement imports yet.</p> :
            <ul aria-label="Statement import queue" className="max-h-96 space-y-2 overflow-y-auto">
              {data.jobs.map(job => {
                const { label, Icon, color } = states[job.status];
                return <li key={job.id} className="min-w-0 space-y-2 rounded-lg border p-3">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <p className="min-w-0 flex-1 break-all text-sm font-medium">{job.filename}</p>
                    <Badge className={color}><Icon aria-hidden="true" className={job.status === 'running' ? 'animate-spin' : undefined} />{label}</Badge>
                  </div>
                  <p className="text-xs text-muted-foreground">Queued {new Date(job.queued_at).toLocaleString()}{job.finished_at ? ` · Finished ${new Date(job.finished_at).toLocaleString()}` : job.started_at ? ` · Started ${new Date(job.started_at).toLocaleString()}` : ''}</p>
                  {job.error && <details className="text-sm text-red-700 dark:text-red-300">
                    <summary className="cursor-pointer">Error details</summary>
                    <p className="mt-2 whitespace-pre-wrap break-words">{job.error}</p>
                  </details>}
                </li>;
              })}
            </ul>}
        </>}
        <details className="text-xs text-muted-foreground">
          <summary className="cursor-pointer">Import notes and partial failures</summary>
          <p className="mt-2">You can add another batch while the queue runs. Queue and results persist across restarts. A failed or interrupted file may have imported some records; review its error before re-uploading. Existing broker importers deduplicate already imported transactions. Files needing interactive confirmation require the CLI. Housekeeping, performance and tax calculations remain separate CLI actions.</p>
        </details>
      </CardContent>
      </Card>
    </div>
  );
};
