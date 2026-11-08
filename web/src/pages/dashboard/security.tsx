import { useEffect, useRef, useState } from 'react';
import { Link } from 'wouter';
import useSwr from 'swr';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import useAuth from '@/hooks/useAuth';
import { BASE_URL, fetcher } from '@/lib/http';
import { ApiTokenScope, type ApiTokensResponse, type CreateApiTokenRequest, type IssuedApiToken, type PasswordChangeRequest, type SessionsResponse } from '@/types/core';
import { OneTimeSecret } from '@/components/composite/account-secret';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

const passwordSchema = yup.object({
  password: yup.string().required('Enter a new password.').test('length', 'Use 12–128 characters and at most 512 UTF-8 bytes.', value => !!value && [...value].length >= 12 && [...value].length <= 128 && new TextEncoder().encode(value).length <= 512),
  confirm: yup.string().required('Confirm the new password.').oneOf([yup.ref('password')], 'Passwords must match.'),
}).required();
const tokenSchema = yup.object({
  name: yup.string().trim().required('Give the token a descriptive name.').max(80, 'Use at most 80 characters.'),
  scope: yup.mixed<ApiTokenScope>().oneOf(Object.values(ApiTokenScope)).required(),
}).required();
const dateLabel = (value: Date) => new Date(String(value)).toLocaleString();

export default function Security() {
  const { auth, transition, requestReauthentication, recoveryLogin, dismissRecoveryLoginNotice } = useAuth();
  const sessions = useSwr<SessionsResponse>(`${BASE_URL}/auth/sessions`, fetcher);
  const tokens = useSwr<ApiTokensResponse>(`${BASE_URL}/auth/api-tokens`, fetcher);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<{ error: boolean; message: string } | null>(null);
  // The one-time token never enters SWR, storage, or the clipboard without a user action.
  const [issuedToken, setIssuedToken] = useState<IssuedApiToken | null>(null);
  const submitting = useRef(false);
  const mounted = useRef(true);
  const passwordForm = useForm<yup.InferType<typeof passwordSchema>>({ resolver: yupResolver(passwordSchema) });
  const tokenForm = useForm<yup.InferType<typeof tokenSchema>>({ resolver: yupResolver(tokenSchema), defaultValues: { name: '', scope: ApiTokenScope.Read } });
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const run = async (action: string, operation: () => Promise<void>, success: string) => {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(action);
    setFeedback(null);
    try {
      await operation();
      if (mounted.current) setFeedback({ error: false, message: success });
    } catch (failure) {
      if (mounted.current && !(failure instanceof Error && failure.name === 'AbortError')) setFeedback({ error: true, message: failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'The request could not be completed. Refresh before retrying.' });
    } finally { submitting.current = false; if (mounted.current) setBusy(null); }
  };
  const changePassword = (fields: yup.InferType<typeof passwordSchema>) => run('password', async () => {
    await transition('/password/change', { new_password: fields.password } satisfies PasswordChangeRequest);
    passwordForm.reset();
  }, 'Password changed. Other sessions and automation tokens have been revoked.');
  const createToken = (fields: yup.InferType<typeof tokenSchema>) => run('token-create', async () => {
    const issued = await transition<IssuedApiToken>('/api-tokens', fields satisfies CreateApiTokenRequest);
    if (!mounted.current) return;
    setIssuedToken(issued);
    tokenForm.reset({ name: '', scope: ApiTokenScope.Read });
    await tokens.mutate();
  }, 'Personal token created. Copy it now; it cannot be retrieved later.');
  return <div className="space-y-6 py-4">
    <header className="space-y-2"><h1 className="text-2xl font-semibold">Account security</h1><p className="text-sm text-muted-foreground">Signed in as <strong>{auth?.user?.username}</strong>. Sensitive changes require your password and an authenticator or unused recovery code within the last five minutes.</p><Button type="button" variant="outline" disabled={!!busy} onClick={requestReauthentication}>Verify identity for sensitive changes</Button><p className="text-xs text-muted-foreground">Verification keeps your drafts. Retry your intended action yourself after verifying; requests are never automatically replayed.</p></header>
    {recoveryLogin && <aside className="space-y-2 rounded-md border p-4"><p className="text-sm">You signed in with a recovery code. If your authenticator is lost, replace it now and save the new recovery codes.</p><Button type="button" variant="outline" onClick={dismissRecoveryLoginNotice}>Dismiss reminder</Button></aside>}
    {feedback && <p role={feedback.error ? 'alert' : 'status'} className={`rounded-md border p-3 text-sm ${feedback.error ? 'text-destructive-foreground' : 'text-muted-foreground'}`}>{feedback.message}</p>}
    <Card><CardHeader><CardTitle>Change password</CardTitle><CardDescription>Your current session is replaced; other sessions and automation tokens are revoked.</CardDescription></CardHeader><CardContent>
      <form className="space-y-4" onSubmit={passwordForm.handleSubmit(changePassword)}>
        <div className="space-y-2"><Label htmlFor="security-password">New password</Label><Input id="security-password" type="password" autoComplete="new-password" {...passwordForm.register('password')} disabled={!!busy} aria-invalid={!!passwordForm.formState.errors.password} /><p className="text-xs text-muted-foreground">12–128 characters and at most 512 UTF-8 bytes. Paste and password managers are supported.</p>{passwordForm.formState.errors.password && <p role="alert" className="text-sm text-destructive-foreground">{passwordForm.formState.errors.password.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="security-confirm">Confirm new password</Label><Input id="security-confirm" type="password" autoComplete="new-password" {...passwordForm.register('confirm')} disabled={!!busy} aria-invalid={!!passwordForm.formState.errors.confirm} />{passwordForm.formState.errors.confirm && <p role="alert" className="text-sm text-destructive-foreground">{passwordForm.formState.errors.confirm.message}</p>}</div>
        <Button type="submit" disabled={!!busy}>{busy === 'password' ? 'Changing password…' : 'Change password'}</Button>
      </form>
    </CardContent></Card>
    <Card><CardHeader><CardTitle>Authenticator and recovery codes</CardTitle><CardDescription>Two-factor authentication is mandatory and cannot be disabled.</CardDescription></CardHeader><CardContent className="space-y-4">
      <p className="text-sm">Replacing your authenticator keeps the old one active until you confirm the new setup. Successful replacement invalidates the old authenticator, recovery codes, other sessions, and automation tokens.</p>
      <Button asChild variant="outline" disabled={!!busy}><Link href="~/enroll?next=%2Fdashboard%2Fsecurity">Replace authenticator</Link></Button>
      <p className="text-sm">Regeneration replaces every recovery code. Save the new set on the next screen before continuing.</p>
      <Button type="button" variant="outline" disabled={!!busy} onClick={() => void run('codes', async () => {
        // AuthProvider retains the one-time response and renders the save screen.
        await transition('/recovery-codes/regenerate', {});
      }, 'Recovery codes regenerated.')}>{busy === 'codes' ? 'Regenerating…' : 'Regenerate recovery codes'}</Button>
    </CardContent></Card>
    <Card><CardHeader><CardTitle>Your browser sessions</CardTitle><CardDescription>Only your unexpired fully authenticated sessions are listed. Revoking this session signs you out immediately.</CardDescription></CardHeader><CardContent className="space-y-3">
      {!sessions.data && !sessions.error && <p role="status">Loading sessions…</p>}
      {sessions.error && <p role="alert" className="text-sm text-destructive-foreground">Session information could not be refreshed. Refresh before revoking a session.</p>}
      <Button type="button" size="sm" variant="outline" disabled={!!busy || sessions.isValidating} onClick={() => void run('sessions-refresh', async () => { await sessions.mutate(); }, 'Sessions refreshed.')}>Refresh sessions</Button>
      {sessions.data && <Table><TableHeader><TableRow><TableHead>Session</TableHead><TableHead>Last active / expiry</TableHead><TableHead className="text-right">Action</TableHead></TableRow></TableHeader><TableBody>
        {sessions.data.sessions.map(session => <TableRow key={session.id}><TableCell className="whitespace-normal"><strong>{session.current ? 'This browser' : 'Other browser session'}</strong><p className="text-xs text-muted-foreground">Created {dateLabel(session.created_at)}</p></TableCell><TableCell className="whitespace-normal text-xs">{dateLabel(session.last_seen_at)}<br />Expires {dateLabel(session.expires_at)}</TableCell><TableCell className="text-right"><Button type="button" size="sm" variant="outline" disabled={!!busy || !!sessions.error} onClick={() => void run(`session:${session.id}`, async () => { await transition(`/sessions/${encodeURIComponent(session.id)}`, undefined, 'DELETE'); if (mounted.current && !session.current) await sessions.mutate(); }, 'Session revoked.')}>{busy === `session:${session.id}` ? 'Revoking…' : session.current ? 'Sign out' : 'Revoke'}</Button></TableCell></TableRow>)}
        {!sessions.data.sessions.length && <TableRow><TableCell colSpan={3}>No active browser sessions.</TableCell></TableRow>}
      </TableBody></Table>}
    </CardContent></Card>
    <Card><CardHeader><CardTitle>Personal API tokens</CardTitle><CardDescription>Tokens expire after 90 days and access only your portfolio. They cannot manage accounts or authorize broker-owner actions.</CardDescription></CardHeader><CardContent className="space-y-4">
      <form className="space-y-3" onSubmit={tokenForm.handleSubmit(createToken)}>
        <div className="space-y-2"><Label htmlFor="token-name">Token name</Label><Input id="token-name" {...tokenForm.register('name')} disabled={!!busy || !!tokens.error} aria-invalid={!!tokenForm.formState.errors.name} />{tokenForm.formState.errors.name && <p role="alert" className="text-sm text-destructive-foreground">{tokenForm.formState.errors.name.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="token-scope">Permission</Label><Select value={tokenForm.watch('scope')} onValueChange={value => tokenForm.setValue('scope', value as ApiTokenScope)} disabled={!!busy || !!tokens.error}><SelectTrigger id="token-scope"><SelectValue /></SelectTrigger><SelectContent><SelectItem value={ApiTokenScope.Read}>Read — view own financial data</SelectItem><SelectItem value={ApiTokenScope.Write}>Write — also change own portfolio</SelectItem></SelectContent></Select></div>
        <p className="text-xs text-muted-foreground">Use read permission unless automation must change your portfolio, import statements, or change your private schedules.</p>
        <Button type="submit" disabled={!!busy || !!tokens.error}>{busy === 'token-create' ? 'Creating…' : 'Create personal token'}</Button>
      </form>
      {issuedToken && <div className="space-y-2"><p className="text-sm">{issuedToken.scope} token · expires {dateLabel(issuedToken.expires_at)}. Copy it now; it is shown only here.</p><OneTimeSecret key={issuedToken.id} title="Personal API token" value={issuedToken.token} onHide={() => setIssuedToken(null)} /></div>}
      {!tokens.data && !tokens.error && <p role="status">Loading personal tokens…</p>}
      {tokens.error && <p role="alert" className="text-sm text-destructive-foreground">Personal tokens could not be refreshed. Refresh before changing tokens.</p>}
      <Button type="button" size="sm" variant="outline" disabled={!!busy || tokens.isValidating} onClick={() => void run('tokens-refresh', async () => { await tokens.mutate(); }, 'Tokens refreshed.')}>Refresh tokens</Button>
      {tokens.data && <Table><TableHeader><TableRow><TableHead>Name / permission</TableHead><TableHead>Expires</TableHead><TableHead className="text-right">Action</TableHead></TableRow></TableHeader><TableBody>
        {tokens.data.tokens.map(token => <TableRow key={token.id}><TableCell className="max-w-48 whitespace-normal break-words">{token.name}<p className="text-xs text-muted-foreground">{token.scope} · created {dateLabel(token.created_at)}</p></TableCell><TableCell className="whitespace-normal text-xs">{dateLabel(token.expires_at)}</TableCell><TableCell className="text-right"><Button type="button" size="sm" variant="outline" disabled={!!busy || !!tokens.error} onClick={() => void run(`token:${token.id}`, async () => { await transition(`/api-tokens/${encodeURIComponent(token.id)}`, undefined, 'DELETE'); if (mounted.current) { setIssuedToken(current => current?.id === token.id ? null : current); await tokens.mutate(); } }, 'Personal token revoked.')}>{busy === `token:${token.id}` ? 'Revoking…' : 'Revoke'}</Button></TableCell></TableRow>)}
        {!tokens.data.tokens.length && <TableRow><TableCell colSpan={3}>No active personal tokens.</TableCell></TableRow>}
      </TableBody></Table>}
    </CardContent></Card>
  </div>;
}
