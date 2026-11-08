import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'wouter';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import useAuth, { safeDashboardPath } from '@/hooks/useAuth';
import { AccountLinkKind, SessionStage, type AccountLinkView, type RegisterRequest } from '@/types/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

const schema = yup.object({
  username: yup.string().required('Choose a username.'),
  password: yup.string().required('Choose a password.').test('length', 'Use 12–128 characters and at most 512 UTF-8 bytes.', value => !!value && [...value].length >= 12 && [...value].length <= 128 && new TextEncoder().encode(value).length <= 512),
  confirm: yup.string().required('Confirm your password.').oneOf([yup.ref('password')], 'Passwords must match.'),
}).required();

const linkHistoryKey = 'doughboxRegisterLinkId';
function rememberLinkId(id: string | null) {
  // Keep router state intact; only the non-secret inspected identity survives reload.
  history.replaceState({ ...history.state, [linkHistoryKey]: id }, '', window.location.pathname + window.location.search);
}

export default function Register() {
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
  const { register, handleSubmit, reset, formState: { errors, isSubmitting } } = useForm<yup.InferType<typeof schema>>({ resolver: yupResolver(schema) });
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
        reset({ username: '', password: '', confirm: '' });
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
      if (mounted.current && generation === bindingGeneration.current && !(failure instanceof Error && failure.name === 'AbortError')) setInspectError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'The invitation could not be inspected.');
    }).finally(() => { if (generation === bindingGeneration.current) inspecting.current = false; });
  }, [status, auth?.stage, auth?.link, transition, inspected, retry]);
  useEffect(() => {
    if (!inspected || !expectedLinkId.current || status !== 'ready') return;
    if (auth?.stage === SessionStage.Anonymous && auth.link?.id === expectedLinkId.current) return;
    setBindingChanged(true);
    setLink(null);
    reset({ username: '', password: '', confirm: '' });
    setError('');
    rememberLinkId(null);
  }, [inspected, status, auth?.stage, auth?.link?.id, reset]);
  useEffect(() => {
    if (status === 'ready' && auth?.stage === SessionStage.EnrollmentRequired) navigate(`/enroll${nextQuery}`, { replace: true });
  }, [status, auth?.stage, navigate, nextQuery]);
  const validLink = status === 'ready' && auth?.stage === SessionStage.Anonymous && inspected && !inspectError && !bindingChanged && link && auth.link?.id === link.id && [AccountLinkKind.Invite, AccountLinkKind.Bootstrap].includes(link.kind);
  const submit = async (fields: yup.InferType<typeof schema>) => {
    if (!validLink || submitting.current) return;
    submitting.current = true;
    setError('');
    try {
      await transition('/register', { link_id: link.id, username: fields.username, password: fields.password } satisfies RegisterRequest);
      reset();
    } catch (failure) {
      if (mounted.current && !(failure instanceof Error && failure.name === 'AbortError')) setError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'Registration failed.');
    } finally { submitting.current = false; }
  };
  return <div className="flex min-h-svh items-center justify-center px-4 py-8"><Card className="w-full max-w-md">
    <CardHeader><CardTitle>Create your account</CardTitle><CardDescription>Registration requires a single-use administrator invitation. An authenticator app is mandatory before your private portfolio is accessible.</CardDescription></CardHeader>
    <CardContent className="space-y-4">
      {status === 'loading' && <p role="status">Checking your session…</p>}
      {status === 'error' && <div role="alert"><p>{authError?.message ?? 'Your session is unavailable.'}</p><Button type="button" variant="outline" onClick={() => void refresh()}>Retry session</Button></div>}
      {status === 'ready' && auth?.stage !== SessionStage.Anonymous && auth?.stage !== SessionStage.EnrollmentRequired && <div className="space-y-3"><p>Sign out of your existing account before using an invitation.</p><Button type="button" variant="outline" onClick={() => void logout()}>Sign out</Button></div>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && !inspected && !inspectError && <p role="status">Inspecting your invitation…</p>}
      {inspectError && <div role="alert" className="space-y-2 text-sm"><p>{inspectError} The link may be expired, revoked, or already used. Ask the administrator for a new invitation.</p><Button type="button" variant="outline" onClick={() => setRetry(value => value + 1)}>Retry inspection</Button></div>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && inspected && !validLink && !inspectError && <p role="alert" className="text-sm">{bindingChanged ? 'The invitation bound to this browser changed. Your draft was cleared. Reopen the original full invitation link from your administrator to continue.' : 'No valid invitation is bound to this tab. Open the full invitation link from your administrator. Expired, revoked, and used links cannot create an account.'}</p>}
      {status === 'ready' && auth?.stage === SessionStage.Anonymous && validLink && <form onSubmit={handleSubmit(submit)} className="space-y-4">
        <p className="text-sm text-muted-foreground">{link.kind === AccountLinkKind.Bootstrap ? 'First administrator setup' : 'Member invitation'} · expires {new Date(String(link.expires_at)).toLocaleString()}</p>
        <div className="space-y-2"><Label htmlFor="register-username">Username</Label><Input id="register-username" autoComplete="username" autoCapitalize="none" spellCheck={false} {...register('username')} disabled={isSubmitting} aria-invalid={!!errors.username} /><p className="text-xs text-muted-foreground">3–32 lowercase letters, numbers, dots, underscores, or hyphens; start with a letter or number. Usernames cannot be changed.</p>{errors.username && <p role="alert" className="text-sm text-destructive-foreground">{errors.username.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="register-password">Password</Label><Input id="register-password" type="password" autoComplete="new-password" {...register('password')} disabled={isSubmitting} aria-invalid={!!errors.password} /><p className="text-xs text-muted-foreground">12–128 characters. Spaces and password-manager paste are supported.</p>{errors.password && <p role="alert" className="text-sm text-destructive-foreground">{errors.password.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="register-confirm">Confirm password</Label><Input id="register-confirm" type="password" autoComplete="new-password" {...register('confirm')} disabled={isSubmitting} aria-invalid={!!errors.confirm} />{errors.confirm && <p role="alert" className="text-sm text-destructive-foreground">{errors.confirm.message}</p>}</div>
        {error && <p role="alert" className="text-sm text-destructive-foreground">{error}</p>}
        <Button type="submit" className="w-full" disabled={isSubmitting}>{isSubmitting ? 'Creating account…' : 'Continue to authenticator setup'}</Button>
      </form>}
      <Link href={`/login${nextQuery}`} className="text-sm underline">Return to sign in</Link>
    </CardContent>
  </Card></div>;
}
