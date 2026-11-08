import { useEffect, useRef, useState, type FormEvent } from 'react';
import useSwr from 'swr';
import useAuth from '@/hooks/useAuth';
import { BASE_URL, fetcher, getPrivateRequestEpoch, isCurrentPrivateRequestEpoch, sendMutateRequest } from '@/lib/http';
import { AccountStatus, InvitesResponse, IssuedAccountLink, Role, UserPatchRequest, UserSummary, UsersResponse } from '@/types/core';
import { OneTimeSecret } from '@/components/composite/account-secret';
import { AdminInstanceSettingsPanel } from '@/components/composite/application-settings';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';

type UserAction = 'role' | 'status' | 'sessions' | 'reset';
type Feedback = { error: boolean; message: string };

const errorMessage = (failure: unknown) => failure && typeof failure === 'object' && 'message' in failure && typeof failure.message === 'string'
  ? failure.message : 'Account action failed. Please retry explicitly.';
const UserCard = ({ user, current, disabled, onAction }: {
  user: UserSummary;
  current: boolean;
  disabled: boolean;
  onAction: (user: UserSummary, action: UserAction, patch?: UserPatchRequest) => Promise<void>;
}) => {
  const [roleDraft, setRoleDraft] = useState<Role | null>(null);
  const [busy, setBusy] = useState<UserAction | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const submitting = useRef(false);
  const run = async (action: UserAction, patch?: UserPatchRequest) => {
    if (submitting.current || disabled) return;
    submitting.current = true;
    setBusy(action);
    setFeedback(null);
    try {
      await onAction(user, action, patch);
      if (action === 'role') setRoleDraft(null);
      setFeedback({ error: false, message: action === 'reset' ? 'Reset link issued below. The existing second factor is still required.' : action === 'sessions' ? 'All account sessions and automation credentials revoked.' : 'Account updated.' });
    } catch (failure) {
      if (failure instanceof Error && failure.name === 'AbortError') return;
      setFeedback({ error: true, message: errorMessage(failure) });
    } finally { submitting.current = false; setBusy(null); }
  };
  const saveRole = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (roleDraft && roleDraft !== user.role) void run('role', { role: roleDraft });
  };
  return <Card>
    <CardHeader>
      <CardTitle className="break-words text-lg">{user.username ?? 'Reserved administrator'}{current && ' (you)'}</CardTitle>
      <CardDescription>Created {new Date(String(user.created_at)).toLocaleString()}</CardDescription>
    </CardHeader>
    <CardContent className="space-y-4">
      <div className="flex flex-wrap gap-2"><Badge variant="outline">{user.role}</Badge><Badge variant="outline">{user.status}</Badge><Badge variant="outline">{user.totp_enabled ? 'Authenticator enrolled' : 'Enrollment incomplete'}</Badge></div>
      <form onSubmit={saveRole} className="space-y-3" aria-label={`Role for ${user.username ?? 'reserved administrator'}`}>
        <div className="max-w-xs space-y-2">
          <Label htmlFor={`role-${user.id}`}>Account role</Label>
          <Select value={roleDraft ?? user.role} disabled={disabled || busy !== null} onValueChange={value => {
            if (value === Role.Admin || value === Role.Member) setRoleDraft(value);
          }}>
            <SelectTrigger id={`role-${user.id}`}><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value={Role.Member}>Member</SelectItem><SelectItem value={Role.Admin}>Administrator</SelectItem></SelectContent>
          </Select>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={disabled || busy !== null || roleDraft === null || roleDraft === user.role}>{busy === 'role' ? 'Saving…' : 'Save role'}</Button>
          {roleDraft !== null && <Button type="button" variant="outline" disabled={disabled || busy !== null} onClick={() => setRoleDraft(null)}>Discard role change</Button>}
        </div>
      </form>
      <p className="text-xs text-muted-foreground">Role and status changes revoke this account's sessions and automation credentials. Re-enabling an incomplete account still requires authenticator enrollment.{current && ' Changing your own role, disabling yourself, or revoking your sessions signs you out.'}</p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" disabled={disabled || busy !== null} onClick={() => void run('status', { disabled: user.status !== AccountStatus.Disabled })}>{busy === 'status' ? 'Updating…' : user.status === AccountStatus.Disabled ? 'Re-enable account' : 'Disable account'}</Button>
        <Button type="button" variant="outline" disabled={disabled || busy !== null} onClick={() => void run('sessions')}>{busy === 'sessions' ? 'Revoking…' : 'Revoke all sessions'}</Button>
        <Button type="button" variant="outline" disabled={disabled || busy !== null || user.status !== AccountStatus.Active || !user.totp_enabled} onClick={() => void run('reset')}>{busy === 'reset' ? 'Issuing…' : 'Issue password-reset link'}</Button>
      </div>
      <p className="text-xs text-muted-foreground">Web password-reset links expire after one hour and require the existing authenticator or an unused recovery code. If both factors are lost, the server operator must use console recovery. The last active administrator cannot be disabled or demoted.</p>
      {feedback && <p role={feedback.error ? 'alert' : 'status'} className="break-words text-sm">{feedback.message}</p>}
    </CardContent>
  </Card>;
};

const AccountManagement = () => {
  const { auth, refresh } = useAuth();
  const { data, error, mutate } = useSwr<UsersResponse>(`${BASE_URL}/admin/users`, fetcher, { refreshInterval: 60_000 });
  const { data: inviteData, error: inviteError, mutate: mutateInvites } = useSwr<InvitesResponse>(`${BASE_URL}/admin/invites`, fetcher, { refreshInterval: 60_000 });
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [secret, setSecret] = useState<{ id: string; title: string; value: string; expiresAt: Date } | null>(null);
  const submitting = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const perform = async <T,>(path: string, method: 'POST' | 'PATCH' | 'DELETE', payload?: UserPatchRequest | Record<string, never>): Promise<T> => {
    if (submitting.current) throw new Error('Another account action is in progress. Please retry when it finishes.');
    submitting.current = true;
    setBusy(true);
    const epoch = getPrivateRequestEpoch();
    try {
      const response = await sendMutateRequest(`${BASE_URL}/admin${path}`, payload, { method });
      const result: T = response.status === 204 ? undefined as T : await response.json();
      if (!mounted.current || !isCurrentPrivateRequestEpoch(epoch)) throw new DOMException('Account state changed.', 'AbortError');
      return result;
    } finally { submitting.current = false; if (mounted.current) setBusy(false); }
  };

  const refreshLists = async () => {
    try { await Promise.all([mutate(), mutateInvites()]); }
    catch (failure) {
      if (failure instanceof Error && failure.name === 'AbortError') return;
      setFeedback({ error: true, message: 'Management lists could not be refreshed. Retry loading before making further changes.' });
    }
  };
  const onUserAction = async (user: UserSummary, action: UserAction, patch?: UserPatchRequest) => {
    const path = `/users/${encodeURIComponent(user.id)}`;
    if (action === 'reset') {
      setSecret(null);
      const issued = await perform<IssuedAccountLink>(`${path}/password-reset-link`, 'POST', {});
      setSecret({ id: issued.id, title: `Password-reset link for ${user.username ?? 'account'}`, value: issued.url, expiresAt: issued.expires_at });
    } else {
      await perform<UserSummary | undefined>(action === 'sessions' ? `${path}/revoke-sessions` : path, action === 'sessions' ? 'POST' : 'PATCH', action === 'sessions' ? {} : patch);
      if (user.id === auth?.user?.id) await refresh();
    }
    await refreshLists();
  };
  const createInvite = async () => {
    setFeedback(null);
    setSecret(null);
    try {
      const issued = await perform<IssuedAccountLink>('/invites', 'POST', {});
      setSecret({ id: issued.id, title: 'Single-use member invitation', value: issued.url, expiresAt: issued.expires_at });
      await refreshLists();
    } catch (failure) {
      if (failure instanceof Error && failure.name === 'AbortError') return;
      setFeedback({ error: true, message: errorMessage(failure) });
    }
  };
  const revokeInvite = async (id: string) => {
    setFeedback(null);
    try {
      await perform<undefined>(`/invites/${encodeURIComponent(id)}`, 'DELETE');
      if (secret?.id === id) setSecret(null);
      setFeedback({ error: false, message: 'Invitation revoked.' });
      await refreshLists();
    } catch (failure) {
      if (failure instanceof Error && failure.name === 'AbortError') return;
      setFeedback({ error: true, message: errorMessage(failure) });
    }
  };

  return <div className="space-y-6">
    <Card>
      <CardHeader><CardTitle>Account administration</CardTitle><CardDescription>Manage access, not portfolios. Administrators cannot impersonate another account or read its financial data.</CardDescription></CardHeader>
      <CardContent className="space-y-3 text-sm">
        <p>Sensitive changes require password and second-factor verification from the last five minutes. If prompted, verify and then explicitly retry your original action; no change is replayed automatically.</p>
        <p>Account role and status decisions, including last-administrator protection, are enforced by the server.</p>
      </CardContent>
    </Card>
    {!data && !error && <p role="status">Loading accounts…</p>}
    {error && <div role="alert" className="space-y-2"><p>Accounts could not be loaded or refreshed. Last loaded accounts, if any, are read-only.</p><Button type="button" variant="outline" disabled={busy} onClick={() => void refreshLists()}>Retry management lists</Button></div>}
    {data?.users.length === 0 && <p>No accounts found.</p>}
    {data?.users.map(user => <UserCard key={user.id} user={user} current={user.id === auth?.user?.id} disabled={busy || !!error} onAction={onUserAction} />)}
    <Card>
      <CardHeader><CardTitle>Member invitations</CardTitle><CardDescription>Share a single-use invitation manually. Links expire after seven days and require username, password, and authenticator enrollment.</CardDescription></CardHeader>
      <CardContent className="space-y-4">
        <Button type="button" disabled={busy || !!inviteError || !inviteData} onClick={() => void createInvite()}>{busy ? 'Working…' : 'Create member invitation'}</Button>
        {!inviteData && !inviteError && <p role="status">Loading invitations…</p>}
        {inviteError && <div role="alert" className="space-y-2"><p>Invitations could not be loaded or refreshed.</p><Button type="button" variant="outline" disabled={busy} onClick={() => void refreshLists()}>Retry management lists</Button></div>}
        {inviteData && <div className="overflow-x-auto"><Table>
          <TableHeader><TableRow><TableHead>Created</TableHead><TableHead>Expires</TableHead><TableHead>Status</TableHead><TableHead>Action</TableHead></TableRow></TableHeader>
          <TableBody>{inviteData.invites.map(invite => {
            const expired = new Date(String(invite.expires_at)).getTime() <= Date.now();
            const active = !invite.consumed_at && !invite.revoked_at && !expired;
            return <TableRow key={invite.id}>
              <TableCell>{new Date(String(invite.created_at)).toLocaleString()}</TableCell>
              <TableCell>{new Date(String(invite.expires_at)).toLocaleString()}</TableCell>
              <TableCell>{invite.consumed_at ? 'Used' : invite.revoked_at ? 'Revoked' : expired ? 'Expired' : 'Available'}</TableCell>
              <TableCell><Button type="button" variant="outline" size="sm" aria-label={`Revoke invitation created ${new Date(String(invite.created_at)).toLocaleString()}`} disabled={busy || !!inviteError || !active} onClick={() => void revokeInvite(invite.id)}>Revoke</Button></TableCell>
            </TableRow>;
          })}</TableBody>
        </Table>{inviteData.invites.length === 0 && <p className="py-3 text-sm text-muted-foreground">No invitations issued.</p>}</div>}
      </CardContent>
    </Card>
    {secret && <section className="space-y-2" aria-label="New account link">
      <p className="text-sm text-muted-foreground">Shown only now. Expires {new Date(String(secret.expiresAt)).toLocaleString()}. Treat this link as a credential and share it only with the intended recipient.</p>
      <OneTimeSecret title={secret.title} value={secret.value} onHide={() => setSecret(null)} />
    </section>}
    {feedback && <p role={feedback.error ? 'alert' : 'status'} className="break-words text-sm">{feedback.message}</p>}
  </div>;
};

const Users = () => {
  const { auth, status } = useAuth();
  if (status !== 'ready' || auth?.stage !== 'authenticated' || auth.user?.role !== Role.Admin) return <p role="status">Administrator access is required.</p>;
  return <Tabs defaultValue="accounts" className="space-y-6 max-md:pb-28">
    <TabsList className="h-auto flex-wrap" aria-label="Administration sections"><TabsTrigger value="accounts" className="min-h-11">Users &amp; invitations</TabsTrigger><TabsTrigger value="instance" className="min-h-11">Instance controls</TabsTrigger></TabsList>
    <TabsContent value="accounts"><AccountManagement /></TabsContent>
    <TabsContent value="instance"><AdminInstanceSettingsPanel /></TabsContent>
  </Tabs>;
};

export default Users;
