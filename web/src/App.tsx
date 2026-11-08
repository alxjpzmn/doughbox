import './index.css';
import { useEffect, useMemo } from 'react';
import { Link, Route, Switch, useLocation, useSearch } from 'wouter';
import { SWRConfig } from 'swr';
import Login from '@/pages/login';
import Register from '@/pages/register';
import Recover from '@/pages/recover';
import Enroll from '@/pages/enroll';
import Overview from '@/pages/dashboard/overview';
import Taxation from '@/pages/dashboard/taxation';
import Instruments from '@/pages/dashboard/instruments';
import Trading from '@/pages/dashboard/trading';
import Security from '@/pages/dashboard/security';
import Users from '@/pages/dashboard/users';
import useIsMobile from '@/hooks/useIsMobile';
import { Menu, MobileMenu } from '@/components/composite/menu';
import useAuth, { AuthProvider, focusedProposalId, safeDashboardPath } from '@/hooks/useAuth';
import { ThemeProvider } from '@/components/theme-provider';
import { ApplicationSettingsPanel } from '@/components/composite/application-settings';
import { Button } from '@/components/ui/button';
import { Role, SessionStage } from '@/types/core';
import { fetcher, isAbortError } from '@/lib/http';

function PrivateDashboard() {
  const isMobile = useIsMobile();
  const { auth, recoveryLogin, dismissRecoveryLoginNotice } = useAuth();
  const cache = useMemo(() => new Map(), []);

  return (
    <SWRConfig value={{
      provider: () => cache,
      fetcher,
      shouldRetryOnError: error => !isAbortError(error) && error.status !== 401 && error.status !== 403,
    }}>
      <div className="fixed inset-0 flex flex-col md:static md:block">
        <main className="min-h-0 flex-1 overflow-y-auto p-6 md:overflow-visible md:px-4 md:py-2 md:pb-safe-or-8">
          <div className="max-w-2xl mx-auto">
            {!isMobile && <Menu />}
            {recoveryLogin && (
              <div role="status" className="my-4 rounded-lg border p-4 text-sm">
                <p>You signed in with a recovery code. If your authenticator is lost, replace it in <Link href="/security" className="underline">Security</Link>.</p>
                <Button type="button" variant="ghost" className="mt-2" onClick={dismissRecoveryLoginNotice}>Dismiss</Button>
              </div>
            )}
            <Switch>
              <Route path="/overview" component={Overview} />
              <Route path="/performance" component={Overview} />
              <Route path="/instruments" component={Instruments} />
              <Route path="/data" component={Instruments} />
              <Route path="/taxation" component={Taxation} />
              <Route path="/trading" component={Trading} />
              <Route path="/settings" component={ApplicationSettingsPanel} />
              <Route path="/security" component={Security} />
              <Route path="/users">
                {auth?.user?.role === Role.Admin ? <Users /> : <p role="alert" className="py-8">Only administrators can manage users and instance controls.</p>}
              </Route>
              <Route><p className="py-8">Page not found. <Link href="/overview" className="underline">Return to your portfolio</Link>.</p></Route>
            </Switch>
          </div>
        </main>
        {isMobile && <MobileMenu />}
      </div>
    </SWRConfig>
  );
}

function ApplicationRoutes() {
  const { auth, status, error, refresh, logout, authenticationEpoch } = useAuth();
  const [location, navigate] = useLocation();
  const search = useSearch();
  const fullyAuthenticated = status === 'ready' && auth?.stage === SessionStage.Authenticated && !!auth.user;
  const accountFlowKey = `${auth?.user?.id ?? 'anonymous'}:${authenticationEpoch}`;

  useEffect(() => {
    if (status !== 'ready' || !auth) return;
    const params = new URLSearchParams(search);
    const proposal = focusedProposalId(search);
    const destination = safeDashboardPath(params.get('next'))
      ?? (proposal ? `/dashboard/trading?proposal=${encodeURIComponent(proposal)}` : null)
      ?? safeDashboardPath(`${location}${search ? `?${search}` : ''}`)
      ?? '/dashboard/overview';
    const withNext = (path: string) => `${path}?next=${encodeURIComponent(destination)}`;
    const dashboard = location === '/dashboard' || location.startsWith('/dashboard/');

    if (auth.stage === SessionStage.EnrollmentRequired && location !== '/enroll') {
      navigate(withNext('/enroll'), { replace: true });
    } else if (auth.stage === SessionStage.MfaRequired && location !== '/login') {
      navigate(withNext('/login'), { replace: true });
    } else if (auth.stage === SessionStage.Anonymous && (dashboard || location === '/enroll' || location === '/')) {
      navigate(withNext('/login'), { replace: true });
    } else if (auth.stage === SessionStage.Authenticated && (location === '/' || location === '/login' || location === '/register' || location === '/recover' || location === '/dashboard')) {
      navigate(destination, { replace: true });
    }
  }, [auth, location, navigate, search, status]);

  if (status === 'loading') {
    return <main className="mx-auto max-w-2xl px-4 py-12"><p role="status">Checking your session…</p></main>;
  }
  if (status === 'error') {
    return (
      <main className="mx-auto max-w-2xl px-4 py-12 space-y-4">
        <h1 className="text-xl font-semibold">Your session could not be verified</h1>
        <p role="alert">{error?.message ?? 'Authentication is temporarily unavailable. Your portfolio is hidden until your session is verified.'}</p>
        <div className="flex flex-wrap gap-2">
          <Button type="button" onClick={() => { void refresh(); }}>Try again</Button>
          <Button type="button" variant="outline" onClick={() => { void logout(); }}>Sign out</Button>
        </div>
      </main>
    );
  }

  return (
    <Switch>
      <Route path="/login"><main className="px-4 py-2 pb-safe-or-32 md:pb-safe-or-8"><div className="max-w-2xl mx-auto"><Login key={accountFlowKey} /></div></main></Route>
      <Route path="/register"><main className="mx-auto max-w-2xl px-4 py-8"><Register key={accountFlowKey} /></main></Route>
      <Route path="/recover"><main className="mx-auto max-w-2xl px-4 py-8"><Recover key={accountFlowKey} /></main></Route>
      <Route path="/enroll"><main className="mx-auto max-w-2xl px-4 py-8">{auth?.stage === SessionStage.EnrollmentRequired || fullyAuthenticated ? <Enroll key={accountFlowKey} /> : <p role="status">Returning to sign in…</p>}</main></Route>
      <Route path="/dashboard" nest>
        {fullyAuthenticated && auth?.user
          ? <PrivateDashboard key={`${auth.user.id}:${authenticationEpoch}`} />
          : <p role="status" className="p-8">Returning to sign in…</p>}
      </Route>
      <Route path="/"><p role="status" className="p-8">Opening Doughbox…</p></Route>
      <Route><main className="mx-auto max-w-2xl px-4 py-12">Page not found. <Link href="/login" className="underline">Sign in</Link>.</main></Route>
    </Switch>
  );
}

export default function App() {
  return (
    <ThemeProvider storageKey="vite-ui-theme">
      <AuthProvider><ApplicationRoutes /></AuthProvider>
    </ThemeProvider>
  );
}
