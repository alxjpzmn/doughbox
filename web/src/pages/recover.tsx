import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'wouter';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import useAuth, { safeDashboardPath } from '@/hooks/useAuth';
import { AccountLinkKind, CodeType, SessionStage, type AccountLinkView, type PasswordResetRequest, type RecoverRequest } from '@/types/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

const schema = yup.object({
  password: yup.string().required('Choose a new password.').test('length', 'Use 12–128 characters and at most 512 UTF-8 bytes.', value => !!value && [...value].length >= 12 && [...value].length <= 128 && new TextEncoder().encode(value).length <= 512),
  confirm: yup.string().required('Confirm your password.').oneOf([yup.ref('password')], 'Passwords must match.'),
  code: yup.string().defined(),
}).required();

const linkHistoryKey = 'doughboxRecoverLinkId';
function rememberLinkId(id: string | null) {
  // Keep router state intact; only the non-secret inspected identity survives reload.
  history.replaceState({ ...history.state, [linkHistoryKey]: id }, '', window.location.pathname + window.location.search);
}

export default function Recover() {
  const { auth, status, error: authError, transition, refresh, logout } = useAuth();
  const [, navigate] = useLocation();
  const token = useRef<string | null>(null);
  const expectedLinkId = useRef<string | null>(null);
  const captured = useRef(false);
  const bindingGeneration = useRef(0);
  const inspecting = useRef(false);
  const mounted = useRef(true);
  const submitting = useRef(false);
  const [inspected, setInspected] = useState(false);
  const [link, setLink] = useState<AccountLinkView | null>(null);
  const [bindingChanged, setBindingChanged] = useState(false);
  const [inspectError, setInspectError] = useState('');
  const [retry, setRetry] = useState(0);
  const [error, setError] = useState('');
  const [codeType, setCodeType] = useState(CodeType.Totp);
  const { register, handleSubmit, reset, setValue, formState: { errors, isSubmitting } } = useForm<yup.InferType<typeof schema>>({ resolver: yupResolver(schema), defaultValues: { code: '' } });
  const next = safeDashboardPath(new URLSearchParams(window.location.search).get('next')) ?? '/dashboard/overview';
  const nextQuery = `?next=${encodeURIComponent(next)}`;
  useEffect(() => {
    mounted.current = true;
    const captureLink = () => {
      const fragment = new URLSearchParams(window.location.hash.slice(1));
      if (captured.current && !fragment.has('token')) return;
      captured.current = true;
      if (fragment.has('token')) {
        token.current = fragment.get('token');
        expectedLinkId.current = null;
        bindingGeneration.current += 1;
        inspecting.current = false;
        setInspected(false);
        setLink(null);
        setBindingChanged(false);
        setInspectError('');
        setError('');
        reset({ password: '', confirm: '', code: '' });
        setCodeType(CodeType.Totp);
        setRetry(value => value + 1);
        rememberLinkId(null);
      } else {
        const savedId: unknown = history.state?.[linkHistoryKey];
        expectedLinkId.current = typeof savedId === 'string' ? savedId : null;
        if (window.location.hash) history.replaceState(history.state, '', window.location.pathname + window.location.search);
      }
    };
    captureLink();
    window.addEventListener('hashchange', captureLink);
    return () => {
      mounted.current = false;
      window.removeEventListener('hashchange', captureLink);
    };
  }, [reset]);
  useEffect(() => {
    if (status !== 'ready' || auth?.stage !== SessionStage.Anonymous || inspecting.current || inspected) return;
    if (!token.current) {
      if (expectedLinkId.current && auth.link?.id === expectedLinkId.current) setLink(auth.link);
      setInspected(true);
      return;
    }
    inspecting.current = true;
    const generation = bindingGeneration.current;
    setInspectError('');
    void transition<AccountLinkView>('/links/inspect', { token: token.current }).then(result => {
      if (mounted.current && generation === bindingGeneration.current) {
        token.current = null;
        expectedLinkId.current = result.id;
        rememberLinkId(result.id);
        setLink(result);
        setInspected(true);
      }
    }).catch(failure => {
      if (mounted.current && generation === bindingGeneration.current && !(failure instanceof Error && failure.name === 'AbortError')) setInspectError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'The recovery link could not be inspected.');
    }).finally(() => { if (generation === bindingGeneration.current) inspecting.current = false; });
  }, [status, auth?.stage, auth?.link, transition, inspected, retry]);
  useEffect(() => {
    if (!inspected || !expectedLinkId.current || status !== 'ready') return;
    if (auth?.stage === SessionStage.Anonymous && auth.link?.id === expectedLinkId.current) return;
    setBindingChanged(true);
    setLink(null);
    reset({ password: '', confirm: '', code: '' });
    setCodeType(CodeType.Totp);
    setError('');
    rememberLinkId(null);
  }, [inspected, status, auth?.stage, auth?.link?.id, reset]);
  useEffect(() => {
    if (status !== 'ready') return;
    if (auth?.stage === SessionStage.EnrollmentRequired) navigate(`/enroll${nextQuery}`, { replace: true });
  }, [status, auth?.stage, navigate, nextQuery]);
  const validLink = status === 'ready' && auth?.stage === SessionStage.Anonymous && inspected && !inspectError && !bindingChanged && link && auth.link?.id === link.id && [AccountLinkKind.PasswordReset, AccountLinkKind.OperatorRecovery].includes(link.kind);
  const operator = link?.kind === AccountLinkKind.OperatorRecovery;
  const submit = async (fields: yup.InferType<typeof schema>) => {
    if (!validLink || submitting.current) return;
    if (!operator && (codeType === CodeType.Totp ? !/^\d{6}$/.test(fields.code) : !fields.code.trim())) { setError(codeType === CodeType.Totp ? 'Enter the six-digit code from your existing authenticator.' : 'Enter an unused recovery code.'); return; }
    submitting.current = true;
    setError('');
    try {
      if (operator) await transition('/recover', { link_id: link.id, new_password: fields.password } satisfies RecoverRequest);
      else await transition('/password/reset', { link_id: link.id, new_password: fields.password, code: fields.code, code_type: codeType } satisfies PasswordResetRequest);
      reset();
      if (!operator) navigate(next, { replace: true });
    } catch (failure) {
      if (mounted.current && !(failure instanceof Error && failure.name === 'AbortError')) setError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'Account recovery failed.');
    } finally { submitting.current = false; }
  };
  return <div className="flex min-h-svh items-center justify-center px-4 py-8"><Card className="w-full max-w-md">
    <CardHeader><CardTitle>Recover your account</CardTitle><CardDescription>Use a single-use recovery link from your administrator or server operator. No email or SMS recovery is available.</CardDescription></CardHeader>
    <CardContent className="space-y-4">
      {status === 'loading' && <p role="status">Checking your session…</p>}
      {status === 'error' && <div role="alert"><p>{authError?.message ?? 'Your session is unavailable.'}</p><Button type="button" variant="outline" onClick={() => void refresh()}>Retry session</Button></div>}
      {status === 'ready' && auth?.stage !== SessionStage.Anonymous && auth?.stage !== SessionStage.EnrollmentRequired && <div className="space-y-3"><p>Sign out of your existing session before using a recovery link.</p><Button type="button" variant="outline" onClick={() => void logout()}>Sign out</Button></div>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && !inspected && !inspectError && <p role="status">Inspecting your recovery link…</p>}
      {inspectError && <div role="alert" className="space-y-2 text-sm"><p>{inspectError} The link may be expired, revoked, or already used. Request another link.</p><Button type="button" variant="outline" onClick={() => setRetry(value => value + 1)}>Retry inspection</Button></div>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && inspected && !validLink && !inspectError && <p role="alert" className="text-sm">{bindingChanged ? 'The recovery link bound to this browser changed. Your password and code drafts were cleared. Reopen the original full recovery link you were given to continue.' : 'No valid recovery link is bound to this tab. Open the full link you were given. If both factors are lost, the server operator must issue a console recovery link.'}</p>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && validLink && <form onSubmit={handleSubmit(submit)} className="space-y-4">
        <p className="text-sm">Account: <strong>{link.username}</strong> · expires {new Date(String(link.expires_at)).toLocaleString()}</p>
        <p className="text-sm text-muted-foreground">{operator ? 'Operator recovery requires a new password and new authenticator enrollment before portfolio access.' : 'This password-reset link does not bypass two-factor authentication. Your existing authenticator or an unused recovery code is still required.'}</p>
        <div className="space-y-2"><Label htmlFor="recover-password">New password</Label><Input id="recover-password" type="password" autoComplete="new-password" {...register('password')} disabled={isSubmitting} aria-invalid={!!errors.password} /><p className="text-xs text-muted-foreground">12–128 characters. Password-manager paste is supported.</p>{errors.password && <p role="alert" className="text-sm text-destructive-foreground">{errors.password.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="recover-confirm">Confirm new password</Label><Input id="recover-confirm" type="password" autoComplete="new-password" {...register('confirm')} disabled={isSubmitting} aria-invalid={!!errors.confirm} />{errors.confirm && <p role="alert" className="text-sm text-destructive-foreground">{errors.confirm.message}</p>}</div>
        {!operator && <><div className="space-y-2"><Label htmlFor="recover-code">{codeType === CodeType.Totp ? 'Existing authenticator code' : 'Unused recovery code'}</Label><Input id="recover-code" type="text" inputMode={codeType === CodeType.Totp ? 'numeric' : 'text'} autoComplete="one-time-code" {...register('code')} disabled={isSubmitting} /></div><Button type="button" variant="link" className="h-auto p-0" disabled={isSubmitting} onClick={() => { setCodeType(codeType === CodeType.Totp ? CodeType.Recovery : CodeType.Totp); setValue('code', ''); setError(''); }}>{codeType === CodeType.Totp ? 'Use a recovery code' : 'Use an authenticator code'}</Button></>}
        {error && <p role="alert" className="text-sm text-destructive-foreground">{error}</p>}
        <Button type="submit" className="w-full" disabled={isSubmitting}>{isSubmitting ? 'Recovering account…' : operator ? 'Continue to new authenticator setup' : 'Reset password and sign in'}</Button>
      </form>}
      <Link href={`/login${nextQuery}`} className="text-sm underline">Return to sign in</Link>
    </CardContent>
  </Card></div>;
}
