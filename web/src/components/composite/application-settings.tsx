import { useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import { CheckCircleIcon, CircleDashedIcon } from '@phosphor-icons/react';
import { ApplicationSettings, ApplicationSettingsStatus, Role, TelegramReportScheduleSettings } from '@/types/core';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import useAuth from '@/hooks/useAuth';
import { PriceStatus } from '@/components/composite/price-status';
import { DataImportStatusPanel } from '@/components/composite/data-import-status';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';

const reportStatuses: Record<string, string> = { not_run: 'Not run', running: 'Running', succeeded: 'Succeeded', failed: 'Failed' };
const logLevels: Record<string, string> = { ERROR: 'Errors only', WARN: 'Warnings and errors', INFO: 'Normal', DEBUG: 'Debug', TRACE: 'Detailed trace' };
type Feedback = { error: boolean; message: string };

const TelegramStatusBadge = ({ isSet }: { isSet: boolean }) => {
  const Icon = isSet ? CheckCircleIcon : CircleDashedIcon;
  return <Badge variant="outline"><Icon aria-hidden="true" />{isSet ? 'Set' : 'Not set'}</Badge>;
};

export const ApplicationSettingsPanel = () => {
  const { data, error, mutate } = useSwr<ApplicationSettingsStatus>(`${BASE_URL}/settings`, fetcher, { refreshInterval: 60_000 });
  const [destinationDraft, setDestinationDraft] = useState<string | null>(null);
  const [scheduleDraft, setScheduleDraft] = useState<TelegramReportScheduleSettings | null>(null);
  const [busy, setBusy] = useState<'destination' | 'schedule' | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const submitting = useRef(false);
  const schedule = scheduleDraft ?? data?.telegram_schedule;
  const scheduleChanged = !!data && !!scheduleDraft && (
    scheduleDraft.enabled !== data.telegram_schedule.enabled || scheduleDraft.send_time_utc !== data.telegram_schedule.send_time_utc
  );
  const destination = destinationDraft ?? data?.telegram_chat_id ?? '';
  const destinationChanged = destinationDraft !== null && destinationDraft.trim() !== (data?.telegram_chat_id ?? '');

  const save = async (event: FormEvent<HTMLFormElement>, section: 'destination' | 'schedule') => {
    event.preventDefault();
    if (!data || error || submitting.current) return;
    if (section === 'schedule' && (!scheduleDraft || !scheduleChanged)) return;
    if (section === 'destination' && !destinationChanged) return;
    if (section === 'schedule' && !/^([01]\d|2[0-3]):[0-5]\d$/.test(scheduleDraft!.send_time_utc)) {
      setFeedback({ error: true, message: 'Choose a daily report time in HH:MM format (UTC).' });
      return;
    }
    submitting.current = true;
    setBusy(section);
    setFeedback(null);
    try {
      const response = await sendMutateRequest(`${BASE_URL}/settings/${section === 'destination' ? 'telegram' : 'telegram-scheduler'}`,
        section === 'destination' ? { chat_id: destination.trim() || null } : scheduleDraft, { method: 'PUT' });
      const saved: ApplicationSettingsStatus = await response.json();
      await mutate(saved, { revalidate: false });
      if (section === 'destination') setDestinationDraft(null);
      else setScheduleDraft(null);
      setFeedback({ error: false, message: section === 'destination' ? 'Your notification destination was saved.' : 'Daily report schedule saved. No restart needed.' });
    } catch (failure) {
      if (failure instanceof Error && failure.name === 'AbortError') return;
      setFeedback({ error: true, message: failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
        ? failure.message : 'Settings could not be saved. Refresh, then retry.' });
    } finally {
      submitting.current = false;
      setBusy(null);
    }
  };

  return <Card>
    <CardHeader><CardTitle>Notification settings</CardTitle></CardHeader>
    <CardContent className="space-y-5">
      {!data && !error && <p role="status" className="text-sm text-muted-foreground">Loading settings…</p>}
      {error && <div role="alert" className="space-y-2 text-sm">
        <p>{data ? 'Settings could not be refreshed. Last loaded values are shown.' : 'Notification settings could not be loaded.'}</p>
        <Button type="button" variant="outline" disabled={busy !== null} onClick={() => void mutate().catch(() => setFeedback({ error: true, message: 'Settings are still unavailable.' }))}>Retry settings</Button>
      </div>}
      {data && <>
        <div className="flex flex-wrap items-center gap-3 text-sm"><span>Instance Telegram bot</span><TelegramStatusBadge isSet={data.telegram_token_set} /></div>
        <p className="text-sm text-muted-foreground">Your chat destination is private to your account. Trading proposal alerts use it immediately; daily portfolio reports follow your schedule below. An administrator must configure the instance bot token before notifications can send.</p>
        <form onSubmit={event => void save(event, 'destination')} aria-label="Telegram notification destination" className="space-y-3">
          <div className="max-w-sm space-y-2">
            <Label htmlFor="telegram-chat-id">Your Telegram chat ID</Label>
            <Input id="telegram-chat-id" value={destination} maxLength={64} autoComplete="off" disabled={busy !== null || !!error} onChange={event => setDestinationDraft(event.target.value)} aria-describedby="telegram-destination-help" />
            <p id="telegram-destination-help" className="text-xs text-muted-foreground">Leave blank and save to disconnect. No other account's destination is used as a fallback.</p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={busy !== null || !!error || !destinationChanged}>{busy === 'destination' ? 'Saving…' : 'Save destination'}</Button>
            {destinationDraft !== null && <Button type="button" variant="outline" disabled={busy !== null} onClick={() => setDestinationDraft(null)}>Discard destination changes</Button>}
          </div>
        </form>
        {(!data.telegram_token_set || !data.telegram_chat_id) && <p role="status" className="text-sm text-muted-foreground">Notifications are unconfigured. You can save a schedule now, but nothing will send until both the bot and your destination are configured.</p>}
        {schedule && <form onSubmit={event => void save(event, 'schedule')} aria-label="Telegram daily report schedule" className="space-y-4 border-t pt-4">
          <div className="flex items-center gap-3">
            <Switch id="telegram-schedule-enabled" checked={schedule.enabled} disabled={busy !== null || !!error} onCheckedChange={enabled => setScheduleDraft({ enabled, send_time_utc: schedule.send_time_utc })} />
            <Label htmlFor="telegram-schedule-enabled">Daily portfolio reports</Label>
          </div>
          <div className="max-w-56 space-y-2">
            <Label htmlFor="telegram-schedule-time">Daily send time (UTC)</Label>
            <Input id="telegram-schedule-time" type="time" step={60} required value={schedule.send_time_utc} disabled={busy !== null || !!error} onChange={event => setScheduleDraft({ enabled: schedule.enabled, send_time_utc: event.target.value })} />
          </div>
          <p className="text-xs text-muted-foreground">Runs daily in UTC while the API is running. Changes apply without restart. Missed reports coalesce into one send; existing external cron jobs are unaffected.</p>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={busy !== null || !!error || !scheduleChanged}>{busy === 'schedule' ? 'Saving…' : 'Save report schedule'}</Button>
            {scheduleDraft !== null && <Button type="button" variant="outline" disabled={busy !== null} onClick={() => setScheduleDraft(null)}>Discard schedule changes</Button>}
          </div>
        </form>}
        <dl className="space-y-2 text-sm">
          <div><dt className="text-muted-foreground">Next report (UTC)</dt><dd>{data.telegram_schedule.next_report_at ? new Date(String(data.telegram_schedule.next_report_at)).toLocaleString(undefined, { timeZone: 'UTC' }) : 'Not scheduled'}</dd></div>
          <div><dt className="text-muted-foreground">Last sent (UTC)</dt><dd>{data.telegram_schedule.last_sent_at ? new Date(String(data.telegram_schedule.last_sent_at)).toLocaleString(undefined, { timeZone: 'UTC' }) : 'Never'}</dd></div>
          <div><dt className="text-muted-foreground">Last report status</dt><dd>{reportStatuses[data.telegram_schedule.last_status] ?? data.telegram_schedule.last_status}</dd></div>
          {data.telegram_schedule.last_error && <div className="break-words"><dt>Last report error</dt><dd>{data.telegram_schedule.last_error}</dd></div>}
        </dl>
      </>}
      {feedback && <p role={feedback.error ? 'alert' : 'status'} className="break-words text-sm">{feedback.message}</p>}
    </CardContent>
  </Card>;
};

const InstanceControls = () => {
  const { data, error, mutate } = useSwr<ApplicationSettings>(`${BASE_URL}/admin/settings`, fetcher, { refreshInterval: 60_000 });
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const submitting = useRef(false);
  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!data || error || !draft || submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFeedback(null);
    try {
      const response = await sendMutateRequest(`${BASE_URL}/admin/settings`, { log_level: draft }, { method: 'PUT' });
      const saved: ApplicationSettings = await response.json();
      await mutate(saved, { revalidate: false });
      setDraft(null);
      setFeedback({ error: false, message: 'Instance log settings saved.' });
    } catch (failure) {
      if (!(failure instanceof Error && failure.name === 'AbortError')) setFeedback({ error: true, message: failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
        ? failure.message : 'Instance settings could not be saved. Refresh, then retry.' });
    } finally { submitting.current = false; setBusy(false); }
  };
  return <section aria-label="Instance controls" className="space-y-6">
    <Card>
      <CardHeader><CardTitle>Instance settings</CardTitle></CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm text-muted-foreground">These administrator controls affect the shared instance. They do not grant access to another account's portfolio.</p>
        {!data && !error && <p role="status">Loading instance settings…</p>}
        {error && <div role="alert" className="space-y-2"><p>Instance settings could not be loaded or refreshed.</p><Button type="button" variant="outline" disabled={busy} onClick={() => void mutate().catch(() => setFeedback({ error: true, message: 'Instance settings are still unavailable.' }))}>Retry instance settings</Button></div>}
        {data && <form onSubmit={event => void save(event)} aria-label="Instance log settings" className="space-y-4">
          <div className="max-w-sm space-y-2">
            <Label htmlFor="application-log-level">Log verbosity</Label>
            <Select value={draft ?? data.log_level} onValueChange={setDraft} disabled={busy || !!error}>
              <SelectTrigger id="application-log-level"><SelectValue /></SelectTrigger>
              <SelectContent>{Object.entries(logLevels).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
            </Select>
          </div>
          <div className="flex flex-wrap gap-2"><Button type="submit" disabled={busy || !!error || draft === null || draft === data.log_level}>{busy ? 'Saving…' : 'Save instance settings'}</Button>{draft !== null && <Button type="button" variant="outline" disabled={busy} onClick={() => setDraft(null)}>Discard changes</Button>}</div>
        </form>}
        {feedback && <p role={feedback.error ? 'alert' : 'status'} className="break-words text-sm">{feedback.message}</p>}
      </CardContent>
    </Card>
    <PriceStatus controls />
    <DataImportStatusPanel />
  </section>;
};

export const AdminInstanceSettingsPanel = () => {
  const { auth, status } = useAuth();
  return status === 'ready' && auth?.stage === 'authenticated' && auth.user?.role === Role.Admin ? <InstanceControls /> : null;
};
