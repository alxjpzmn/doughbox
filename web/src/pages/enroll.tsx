import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'wouter';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import useAuth, { safeDashboardPath } from '@/hooks/useAuth';
import { SessionStage, type PendingTotpRequest, type TotpConfirmRequest, type TotpStartResponse } from '@/types/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

const schema = yup.object({ code: yup.string().required('Enter your authenticator code.').matches(/^\d{6}$/, 'Enter all six digits, including any leading zeroes.') }).required();

export default function Enroll() {
  const { auth, status, error: authError, transition, refresh, requestReauthentication, logout } = useAuth();
  const [, navigate] = useLocation();
  const [enrollment, setEnrollment] = useState<TotpStartResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [now, setNow] = useState(Date.now());
  const submitting = useRef(false);
  const mounted = useRef(true);
  const { register, handleSubmit, reset, formState: { errors } } = useForm<yup.InferType<typeof schema>>({ resolver: yupResolver(schema) });
  const next = safeDashboardPath(new URLSearchParams(window.location.search).get('next')) ?? '/dashboard/overview';
  const nextQuery = `?next=${encodeURIComponent(next)}`;
  const replacement = auth?.stage === SessionStage.Authenticated;
  const permitted = status === 'ready' && (replacement || auth?.stage === SessionStage.EnrollmentRequired);
  const expired = !!enrollment && new Date(String(enrollment.expires_at)).getTime() <= now;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (status === 'ready' && !permitted) navigate(`/login${nextQuery}`, { replace: true });
  }, [status, permitted, navigate, nextQuery]);
  useEffect(() => {
    if (auth?.stage !== SessionStage.EnrollmentRequired) return;
    const timer = window.setTimeout(() => { void refresh(); }, Math.max(0, new Date(String(auth.expires_at)).getTime() - Date.now()) + 100);
    return () => window.clearTimeout(timer);
  }, [auth, refresh]);
  const run = async (operation: () => Promise<void>) => {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError('');
    try { await operation(); }
    catch (failure) {
      if (mounted.current && !(failure instanceof Error && failure.name === 'AbortError')) setError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'Authenticator setup is unavailable. Please try again.');
    } finally { submitting.current = false; if (mounted.current) setBusy(false); }
  };
  const start = () => run(async () => {
    // Starting again intentionally invalidates the previous pending enrollment ID.
    setEnrollment(null);
    reset();
    const result = await transition<TotpStartResponse>('/totp/start', {});
    if (mounted.current) { setEnrollment(result); setNow(Date.now()); }
  });
  const cancel = () => run(async () => {
    if (!enrollment) return;
    await transition('/totp/pending', { enrollment_id: enrollment.enrollment_id } satisfies PendingTotpRequest, 'DELETE');
    if (mounted.current) { setEnrollment(null); reset(); }
  });
  const confirm = (fields: yup.InferType<typeof schema>) => run(async () => {
    if (!enrollment || expired) return;
    // Recovery codes from this response belong exclusively to AuthProvider.
    await transition('/totp/confirm', { enrollment_id: enrollment.enrollment_id, code: fields.code } satisfies TotpConfirmRequest);
    if (mounted.current) { setEnrollment(null); reset(); }
  });
  return <div className="flex min-h-svh items-center justify-center px-4 py-8"><Card className="w-full max-w-lg">
    <CardHeader><CardTitle>{replacement ? 'Replace your authenticator' : 'Set up your authenticator'}</CardTitle><CardDescription>{replacement ? 'Your current authenticator stays active until the new one is confirmed.' : 'Two-factor authentication is required. Your portfolio remains locked until setup is complete.'}</CardDescription></CardHeader>
    <CardContent className="space-y-4">
      {status === 'loading' && <p role="status">Checking enrollment authority…</p>}
      {status === 'error' && <div role="alert"><p>{authError?.message ?? 'Your session could not be checked.'}</p><Button type="button" variant="outline" onClick={() => void refresh()}>Retry session</Button></div>}
      {permitted && <>
        <p className="text-sm">Issuer: <strong>Doughbox</strong> · Account: <strong>{auth?.user?.username}</strong></p>
        {replacement && <div className="space-y-2"><p className="text-sm text-muted-foreground">Replacement requires verification within the last five minutes. After verification, choose Start setup again. Successful replacement revokes other sessions and tokens and replaces your recovery codes.</p><Button type="button" variant="outline" disabled={busy} onClick={requestReauthentication}>Verify identity</Button></div>}
        {!enrollment && <><p className="text-sm">Use any compatible authenticator app. The QR code is generated by this server, not by an external QR service.</p><Button type="button" disabled={busy} onClick={start}>{busy ? 'Preparing…' : 'Start authenticator setup'}</Button></>}
        {enrollment && <div className="space-y-4">
          <img className="mx-auto h-auto w-64 max-w-full rounded-md bg-white p-3" src={`data:image/png;base64,${enrollment.qr_png_base64}`} alt={`Authenticator setup QR code for Doughbox account ${auth?.user?.username ?? ''}`} />
          <div className="space-y-2"><Label htmlFor="enrollment-secret">Manual setup key</Label><Input id="enrollment-secret" className="font-mono" readOnly value={enrollment.secret_base32} onFocus={event => event.currentTarget.select()} spellCheck={false} /><p className="text-xs text-muted-foreground">If you cannot scan the image, enter this key in your app with a time-based, six-digit SHA-1 code and a 30-second period. Keep this key private.</p></div>
          <p className="text-sm" role="status">{expired ? 'This setup has expired. Start again for a new key.' : `Confirm before ${new Date(String(enrollment.expires_at)).toLocaleTimeString()}.`}</p>
          <p className="text-xs text-muted-foreground">Set your device clock to automatic. Codes change every 30 seconds. Paste is supported; include leading zeroes.</p>
          <form className="space-y-3" onSubmit={handleSubmit(confirm)}>
            <Label htmlFor="enrollment-code">Six-digit code from the new authenticator</Label>
            <Input id="enrollment-code" type="text" inputMode="numeric" autoComplete="one-time-code" {...register('code')} disabled={busy || expired} aria-invalid={!!errors.code} />
            {errors.code && <p role="alert" className="text-sm text-destructive-foreground">{errors.code.message}</p>}
            <Button type="submit" className="w-full" disabled={busy || expired}>{busy ? 'Confirming…' : 'Confirm authenticator'}</Button>
          </form>
          <div className="flex flex-wrap gap-2"><Button type="button" variant="outline" disabled={busy} onClick={start}>{expired ? 'Start again' : 'Generate a different setup key'}</Button><Button type="button" variant="outline" disabled={busy} onClick={cancel}>Cancel pending setup</Button></div>
        </div>}
        {error && <p role="alert" className="text-sm text-destructive-foreground">{error}</p>}
        {replacement ? <Link href="/dashboard/security" className="block text-sm underline">Back to security settings</Link> : <Button type="button" variant="link" className="h-auto" disabled={busy} onClick={() => void run(logout)}>Sign out and resume later</Button>}
        <p className="text-xs text-muted-foreground">After confirmation, save the one-time recovery codes before continuing. If you leave initial enrollment unfinished, resume by signing in with your chosen username and password.</p>
      </>}
    </CardContent>
  </Card></div>;
}
