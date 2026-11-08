import { useEffect, useRef, useState } from 'react';
import { useLocation } from 'wouter';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import useAuth, { focusedProposalId, safeDashboardPath } from '@/hooks/useAuth';
import { CodeType, SessionStage, type LoginRequest, type VerificationRequest } from '@/types/core';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

const passwordSchema = yup.object({
  username: yup.string().required('Enter your username.'),
  password: yup.string().required('Enter your password.'),
}).required();
const codeSchema = yup.object({ code: yup.string().required('Enter your verification code.') }).required();

export default function Login() {
  const { auth, status, error: authError, transition, refresh, logout } = useAuth();
  const [, navigate] = useLocation();
  const [codeType, setCodeType] = useState(CodeType.Totp);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const mounted = useRef(true);
  const proposalId = focusedProposalId();
  const next = safeDashboardPath(new URLSearchParams(window.location.search).get('next'))
    ?? (proposalId ? `/dashboard/trading?proposal=${encodeURIComponent(proposalId)}` : null)
    ?? '/dashboard/overview';
  const passwordForm = useForm<yup.InferType<typeof passwordSchema>>({ resolver: yupResolver(passwordSchema) });
  const codeForm = useForm<yup.InferType<typeof codeSchema>>({ resolver: yupResolver(codeSchema) });
  const challenge = auth?.stage === SessionStage.MfaRequired;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (status !== 'ready') return;
    if (auth?.stage === SessionStage.Authenticated) navigate(next, { replace: true });
    if (auth?.stage === SessionStage.EnrollmentRequired) navigate(`/enroll?next=${encodeURIComponent(next)}`, { replace: true });
  }, [auth?.stage, status, navigate, next]);
  useEffect(() => {
    if (!challenge || !auth) return;
    const timer = window.setTimeout(() => { void refresh(); }, Math.max(0, new Date(String(auth.expires_at)).getTime() - Date.now()) + 100);
    return () => window.clearTimeout(timer);
  }, [auth, challenge, refresh]);
  useEffect(() => { setError(''); codeForm.reset(); }, [auth?.stage, codeForm.reset]);
  const run = async (operation: () => Promise<unknown>) => {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError('');
    try { await operation(); }
    catch (failure) {
      if (mounted.current && !(failure instanceof Error && failure.name === 'AbortError')) setError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'Sign in is unavailable. Please try again.');
    } finally { submitting.current = false; if (mounted.current) setBusy(false); }
  };
  return <div className="flex min-h-svh items-center justify-center px-4 py-8">
    <Card className="w-full max-w-sm">
      <CardHeader><CardTitle>{challenge ? 'Two-factor verification' : 'Sign in to Doughbox'}</CardTitle><CardDescription>{challenge ? `Enter a code for ${auth?.user?.username ?? 'your account'}.` : 'Use your username, password, and authenticator app.'}</CardDescription></CardHeader>
      <CardContent className="space-y-4">
        {status === 'loading' && <p role="status">Checking your session…</p>}
        {status === 'error' && <div role="alert" className="space-y-2"><p>{authError?.message ?? 'Your session could not be checked.'}</p><Button type="button" variant="outline" onClick={() => void refresh()}>Retry session</Button></div>}
        {status === 'ready' && auth?.setup_required && <p className="text-sm text-muted-foreground">This installation needs its first administrator. Ask the server operator for the console-generated bootstrap link. This page cannot create an account without a valid link.</p>}
        {status === 'ready' && !challenge && auth?.stage === SessionStage.Anonymous && <form className="space-y-4" onSubmit={passwordForm.handleSubmit(fields => run(async () => {
          await transition('/login', fields satisfies LoginRequest);
          passwordForm.reset({ username: fields.username, password: '' });
        }))}>
          <div className="space-y-2"><Label htmlFor="login-username">Username</Label><Input id="login-username" autoComplete="username" autoCapitalize="none" spellCheck={false} {...passwordForm.register('username')} disabled={busy} aria-invalid={!!passwordForm.formState.errors.username} />{passwordForm.formState.errors.username && <p role="alert" className="text-sm text-destructive-foreground">{passwordForm.formState.errors.username.message}</p>}</div>
          <div className="space-y-2"><Label htmlFor="login-password">Password</Label><Input id="login-password" type="password" autoComplete="current-password" {...passwordForm.register('password')} disabled={busy} aria-invalid={!!passwordForm.formState.errors.password} />{passwordForm.formState.errors.password && <p role="alert" className="text-sm text-destructive-foreground">{passwordForm.formState.errors.password.message}</p>}</div>
          <Button className="w-full" type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</Button>
          <p className="text-xs text-muted-foreground">New accounts require an administrator-issued invitation. If you have lost both factors, contact your server operator for recovery.</p>
        </form>}
        {status === 'ready' && challenge && <form className="space-y-4" onSubmit={codeForm.handleSubmit(fields => run(async () => {
          if (codeType === CodeType.Totp && !/^\d{6}$/.test(fields.code)) { setError('Enter the six-digit code from your authenticator.'); return; }
          await transition('/mfa/verify', { code: fields.code, code_type: codeType } satisfies VerificationRequest);
          codeForm.reset();
        }))}>
          <div className="space-y-2"><Label htmlFor="login-code">{codeType === CodeType.Totp ? 'Six-digit authenticator code' : 'Unused recovery code'}</Label><Input id="login-code" type="text" inputMode={codeType === CodeType.Totp ? 'numeric' : 'text'} autoComplete="one-time-code" {...codeForm.register('code')} disabled={busy} aria-invalid={!!codeForm.formState.errors.code} />{codeForm.formState.errors.code && <p role="alert" className="text-sm text-destructive-foreground">{codeForm.formState.errors.code.message}</p>}</div>
          <p className="text-xs text-muted-foreground">This password challenge expires at {new Date(String(auth?.expires_at)).toLocaleTimeString()}. If it expires, sign in again with your password.</p>
          <Button type="button" variant="link" className="h-auto p-0" disabled={busy} onClick={() => { setCodeType(codeType === CodeType.Totp ? CodeType.Recovery : CodeType.Totp); codeForm.reset(); setError(''); }}>{codeType === CodeType.Totp ? 'Use a recovery code' : 'Use an authenticator code'}</Button>
          <Button className="w-full" type="submit" disabled={busy}>{busy ? 'Verifying…' : 'Verify and sign in'}</Button>
          <Button className="w-full" type="button" variant="outline" disabled={busy} onClick={() => void run(logout)}>Back to password sign in</Button>
        </form>}
        {error && <p role="alert" className="text-sm text-destructive-foreground">{error}</p>}
      </CardContent>
    </Card>
  </div>;
}
