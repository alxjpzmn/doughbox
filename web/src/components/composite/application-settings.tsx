import { useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import { ApplicationSettings } from '@/types/core';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

const logLevels: Record<string, string> = {
  ERROR: 'Errors only',
  WARN: 'Warnings and errors',
  INFO: 'Normal',
  DEBUG: 'Debug',
  TRACE: 'Detailed trace',
};

export const ApplicationSettingsPanel = () => {
  const { data, error, mutate } = useSwr<ApplicationSettings>(`${BASE_URL}/settings`, fetcher);
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ error: boolean; message: string } | null>(null);
  const submitting = useRef(false);

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!data || !draft || submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFeedback(null);
    try {
      const response = await sendMutateRequest(`${BASE_URL}/settings`, { log_level: draft }, { method: 'PUT' });
      const saved: ApplicationSettings = await response.json();
      await mutate(saved, { revalidate: false });
      setDraft(null);
      setFeedback({ error: false, message: 'Application settings saved.' });
    } catch (failure) {
      const message = failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
        ? failure.message : 'Application settings could not be saved. Refresh before retrying.';
      setFeedback({ error: true, message });
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };

  return <Card>
    <CardHeader>
      <CardTitle>Application settings</CardTitle>
    </CardHeader>
    <CardContent className="space-y-4">
      {!data && !error && <p role="status" className="text-sm text-muted-foreground">Loading settings…</p>}
      {error && <div role="alert" className="space-y-2 text-sm text-destructive">
        <p>{data ? 'Settings could not be refreshed. Last loaded values are shown.' : 'Application settings could not be loaded.'}</p>
        <Button type="button" variant="outline" disabled={busy} onClick={async () => {
          try { await mutate(); } catch { setFeedback({ error: true, message: 'Settings are still unavailable.' }); }
        }}>Retry settings</Button>
      </div>}
      {data && <form onSubmit={save} aria-label="Application settings" className="space-y-4">
        <div className="max-w-sm space-y-2">
          <Label htmlFor="application-log-level">Log verbosity</Label>
          <Select value={draft ?? data.log_level} onValueChange={setDraft} disabled={busy || !!error}>
            <SelectTrigger id="application-log-level"><SelectValue /></SelectTrigger>
            <SelectContent>{Object.entries(logLevels).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={busy || !!error || draft === null || draft === data.log_level}>{busy ? 'Saving…' : 'Save settings'}</Button>
          {draft !== null && <Button type="button" variant="outline" disabled={busy} onClick={() => setDraft(null)}>Discard changes</Button>}
        </div>
      </form>}
      {feedback && <p role={feedback.error ? 'alert' : 'status'} className={`text-sm ${feedback.error ? 'text-destructive' : 'text-muted-foreground'}`}>{feedback.message}</p>}
    </CardContent>
  </Card>;
};
