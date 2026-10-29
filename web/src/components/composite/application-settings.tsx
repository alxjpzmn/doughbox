import { useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import { CheckCircleIcon, CircleDashedIcon } from '@phosphor-icons/react';
import { ApplicationSettingsStatus, TelegramReportScheduleSettings } from '@/types/core';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

const reportStatuses: Record<string, string> = {
  not_run: 'Not run',
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
};
const logLevels: Record<string, string> = {
  ERROR: 'Errors only',
  WARN: 'Warnings and errors',
  INFO: 'Normal',
  DEBUG: 'Debug',
  TRACE: 'Detailed trace',
};

const TelegramStatusBadge = ({ isSet }: { isSet: boolean }) => {
  const Icon = isSet ? CheckCircleIcon : CircleDashedIcon;
  return <Badge variant="outline" className={isSet
    ? 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900 dark:text-green-100'
    : 'bg-muted text-muted-foreground'}>
    <Icon aria-hidden="true" />
    {isSet ? 'Set' : 'Not set'}
  </Badge>;
};

export const ApplicationSettingsPanel = () => {
  const { data, error, mutate } = useSwr<ApplicationSettingsStatus>(`${BASE_URL}/settings`, fetcher, { refreshInterval: 60_000 });
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ error: boolean; message: string } | null>(null);
  const submitting = useRef(false);
  const [scheduleDraft, setScheduleDraft] = useState<TelegramReportScheduleSettings | null>(null);
  const [scheduleBusy, setScheduleBusy] = useState(false);
  const [scheduleFeedback, setScheduleFeedback] = useState<{ error: boolean; message: string } | null>(null);
  const scheduleSubmitting = useRef(false);
  const schedule = scheduleDraft ?? data?.telegram_schedule;
  const scheduleChanged = !!data && !!scheduleDraft && (
    scheduleDraft.enabled !== data.telegram_schedule.enabled ||
    scheduleDraft.send_time_utc !== data.telegram_schedule.send_time_utc
  );

  const saveSchedule = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!data || error || !scheduleDraft || !scheduleChanged || busy || scheduleSubmitting.current) return;
    if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(scheduleDraft.send_time_utc)) {
      setScheduleFeedback({ error: true, message: 'Choose a daily report time in HH:MM format (UTC).' });
      return;
    }
    scheduleSubmitting.current = true;
    setScheduleBusy(true);
    setScheduleFeedback(null);
    try {
      const response = await sendMutateRequest(`${BASE_URL}/settings/telegram-scheduler`, scheduleDraft, { method: 'PUT' });
      const saved: ApplicationSettingsStatus = await response.json();
      await mutate(saved, { revalidate: false });
      setScheduleDraft(null);
      setScheduleFeedback({ error: false, message: 'Daily report schedule saved. No restart needed.' });
    } catch (failure) {
      const message = failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
        ? failure.message : 'Schedule could not be saved. Refresh settings, then retry.';
      setScheduleFeedback({ error: true, message });
    } finally {
      scheduleSubmitting.current = false;
      setScheduleBusy(false);
    }
  };

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!data || error || !draft || submitting.current || scheduleSubmitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFeedback(null);
    try {
      const response = await sendMutateRequest(`${BASE_URL}/settings`, { log_level: draft }, { method: 'PUT' });
      const saved: ApplicationSettingsStatus = await response.json();
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
          <Select value={draft ?? data.log_level} onValueChange={setDraft} disabled={busy || scheduleBusy || !!error}>
            <SelectTrigger id="application-log-level"><SelectValue /></SelectTrigger>
            <SelectContent>{Object.entries(logLevels).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={busy || scheduleBusy || !!error || draft === null || draft === data.log_level}>{busy ? 'Saving…' : 'Save settings'}</Button>
          {draft !== null && <Button type="button" variant="outline" disabled={busy || scheduleBusy} onClick={() => setDraft(null)}>Discard changes</Button>}
        </div>
      </form>}
      {data && <section aria-labelledby="telegram-settings-title" className="space-y-3 border-t pt-4">
        <h3 id="telegram-settings-title" className="text-sm font-semibold">Telegram notifications</h3>
        <dl className="max-w-sm space-y-2 text-sm">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <dt>Bot token <code className="text-xs text-muted-foreground">TG_TOKEN</code></dt>
            <dd><TelegramStatusBadge isSet={data.telegram_token_set} /></dd>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <dt>Chat ID <code className="text-xs text-muted-foreground">TG_CHAT_ID</code></dt>
            <dd><TelegramStatusBadge isSet={data.telegram_chat_id_set} /></dd>
          </div>
        </dl>
        <p className="text-xs text-muted-foreground">Both values must be set on the server to send Telegram notifications. Values are never shown here; this status does not verify them with Telegram.</p>
        {(!data.telegram_token_set || !data.telegram_chat_id_set) && <p role="status" className="text-sm text-muted-foreground">You can save a schedule now, but reports will not send until both Telegram values are configured.</p>}
        {schedule && <form onSubmit={saveSchedule} aria-label="Telegram daily report schedule" className="space-y-4">
          <div className="flex items-center gap-3">
            <Switch id="telegram-schedule-enabled" checked={schedule.enabled} disabled={scheduleBusy || busy || !!error}
              onCheckedChange={enabled => {
                setScheduleDraft({ enabled, send_time_utc: schedule.send_time_utc });
                setScheduleFeedback(null);
              }} />
            <Label htmlFor="telegram-schedule-enabled">Daily portfolio reports</Label>
          </div>
          <div className="max-w-56 space-y-2">
            <Label htmlFor="telegram-schedule-time">Daily send time (UTC)</Label>
            <Input id="telegram-schedule-time" type="time" step={60} required value={schedule.send_time_utc}
              disabled={scheduleBusy || busy || !!error} aria-describedby="telegram-schedule-help"
              onChange={event => {
                setScheduleDraft({ enabled: schedule.enabled, send_time_utc: event.target.value });
                setScheduleFeedback(null);
              }} />
          </div>
          <p id="telegram-schedule-help" className="text-xs text-muted-foreground">Runs daily in UTC while the API is running. Changes apply without restart. Missed reports coalesce into one send; existing external cron jobs are unaffected.</p>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={scheduleBusy || busy || !!error || !scheduleChanged}>{scheduleBusy ? 'Saving…' : 'Save report schedule'}</Button>
            {scheduleDraft !== null && <Button type="button" variant="outline" disabled={scheduleBusy} onClick={() => {
              setScheduleDraft(null);
              setScheduleFeedback(null);
            }}>Discard schedule changes</Button>}
          </div>
          {scheduleFeedback && <p role={scheduleFeedback.error ? 'alert' : 'status'} className={`text-sm ${scheduleFeedback.error ? 'text-destructive' : 'text-muted-foreground'}`}>{scheduleFeedback.message}</p>}
        </form>}
        <dl className="space-y-2 text-sm">
          <div className="flex flex-wrap gap-x-3 gap-y-1">
            <dt className="text-muted-foreground">Next report (UTC)</dt>
            <dd>{data.telegram_schedule.next_report_at ? new Date(data.telegram_schedule.next_report_at).toLocaleString(undefined, { timeZone: 'UTC' }) : 'Not scheduled'}</dd>
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-1">
            <dt className="text-muted-foreground">Last sent (UTC)</dt>
            <dd>{data.telegram_schedule.last_sent_at ? new Date(data.telegram_schedule.last_sent_at).toLocaleString(undefined, { timeZone: 'UTC' }) : 'Never'}</dd>
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-1">
            <dt className="text-muted-foreground">Last report status</dt>
            <dd>{reportStatuses[data.telegram_schedule.last_status] ?? data.telegram_schedule.last_status}</dd>
          </div>
          {data.telegram_schedule.last_error && <div className="space-y-1 break-words text-destructive">
            <dt>Last report error</dt>
            <dd>{data.telegram_schedule.last_error}</dd>
          </div>}
        </dl>
      </section>}
      {feedback && <p role={feedback.error ? 'alert' : 'status'} className={`text-sm ${feedback.error ? 'text-destructive' : 'text-muted-foreground'}`}>{feedback.message}</p>}
    </CardContent>
  </Card>;
};
