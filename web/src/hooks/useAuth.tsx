import {
  BASE_URL,
  abortError,
  apiFetch,
  beginHttpAuthTransition,
  fetchAuthState,
  getPrivateRequestEpoch,
  invalidatePrivateRequests,
  isAbortError,
  sendMutateRequest,
  setHttpAuthBoundary,
  setHttpCsrfToken,
  toHttpError,
  type HttpError,
} from '@/lib/http';
import { SessionStage, type AuthState, type ReauthRequest, type RecoveryCodesResponse } from '@/types/core';
import { RecoveryCodesScreen } from '@/components/composite/recovery-codes';
import { ReauthenticationDialog } from '@/components/composite/reauth-dialog';
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { useLocation } from 'wouter';

const proposalIdPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const rotationPaths: Record<string, true> = {
  '/login': true,
  '/mfa/verify': true,
  '/register': true,
  '/recover': true,
  '/password/reset': true,
  '/totp/confirm': true,
  '/reauth': true,
  '/password/change': true,
  '/recovery-codes/regenerate': true,
};

export function focusedProposalId(search = window.location.search): string | null {
  const id = new URLSearchParams(search).get('proposal');
  return id && proposalIdPattern.test(id) ? id : null;
}

export function safeDashboardPath(value: string | null): string | null {
  if (!value?.startsWith('/dashboard/')) return null;
  let decoded = value;
  try {
    // Reject encoded traversal/separators as well as their literal forms.
    while (decoded.includes('%')) {
      const next = decodeURIComponent(decoded);
      if (next === decoded) break;
      decoded = next;
    }
  } catch {
    return null;
  }
  if (/[\\\u0000-\u0020\u007f#]/.test(decoded) || decoded.includes('..') || decoded.includes('//') || decoded.includes('://')) return null;
  return decoded.startsWith('/dashboard/') ? value : null;
}

interface AuthContextValue {
  auth: AuthState | null;
  status: 'loading' | 'ready' | 'error';
  error: HttpError | null;
  authenticationEpoch: number;
  refresh: () => Promise<AuthState | undefined>;
  transition: <T = AuthState>(path: string, payload?: unknown, method?: 'POST' | 'PUT' | 'PATCH' | 'DELETE') => Promise<T>;
  logout: () => Promise<void>;
  requestReauthentication: () => void;
  recoveryLogin: boolean;
  dismissRecoveryLoginNotice: () => void;
  acknowledgeRecoveryCodes: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

function isAuthState(value: unknown): value is AuthState {
  return typeof value === 'object' && value !== null && 'stage' in value && 'csrf_token' in value;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [, navigate] = useLocation();
  const [auth, setAuth] = useState<AuthState | null>(null);
  const [status, setStatus] = useState<AuthContextValue['status']>('loading');
  const [error, setError] = useState<HttpError | null>(null);
  const [authenticationEpoch, setAuthenticationEpoch] = useState(getPrivateRequestEpoch);
  const [recoveryCodes, setRecoveryCodes] = useState<readonly string[] | null>(null);
  const [recoveryLogin, setRecoveryLogin] = useState(false);
  const [reauthenticationOpen, setReauthenticationOpen] = useState(false);
  const authRef = useRef<AuthState | null>(null);
  const identityRef = useRef<string | null>(null);
  const generationRef = useRef(0);
  const mountedRef = useRef(false);
  const actionsInFlightRef = useRef(0);
  const channelRef = useRef<BroadcastChannel | null>(null);
  const recoveryDestinationRef = useRef<string | null>(null);
  const logoutBarrierRef = useRef(false);
  const logoutCsrfRef = useRef<string | null>(null);

  const invalidatePrivate = useCallback(() => {
    setAuthenticationEpoch(invalidatePrivateRequests());
  }, []);

  const hideAuthority = useCallback(() => {
    invalidatePrivate();
    authRef.current = null;
    identityRef.current = null;
    setHttpCsrfToken(null);
    setAuth(null);
    setRecoveryCodes(null);
    setRecoveryLogin(false);
    setReauthenticationOpen(false);
  }, [invalidatePrivate]);

  const installAuth = useCallback((next: AuthState) => {
    const identity = next.stage === SessionStage.Authenticated && next.user
      ? `${next.user.id}:${next.user.auth_version}:${next.user.role}`
      : null;
    if (identityRef.current !== identity) {
      invalidatePrivate();
      setRecoveryCodes(null);
      setRecoveryLogin(false);
      setReauthenticationOpen(false);
    }
    if (authRef.current?.user?.id !== next.user?.id) setRecoveryCodes(null);
    identityRef.current = identity;
    authRef.current = next;
    setHttpCsrfToken(next.csrf_token);
    setAuth(next);
    setStatus('ready');
    setError(null);
  }, [invalidatePrivate]);

  const failAuth = useCallback((failure: unknown) => {
    hideAuthority();
    setStatus('error');
    setError(toHttpError(failure));
  }, [hideAuthority]);

  const readState = useCallback(async (generation: number): Promise<AuthState> => {
    const next = await fetchAuthState();
    if (!mountedRef.current || generation !== generationRef.current) throw abortError();
    // Do not restore an identity while a deliberate logout remains unconfirmed.
    if (logoutBarrierRef.current && next.stage !== SessionStage.Anonymous) {
      logoutCsrfRef.current = next.csrf_token;
      throw {
        status: 409,
        error: 'AuthenticationStateChanged',
        message: 'Sign-out could not be confirmed. Sign out again before continuing.',
      } satisfies HttpError;
    }
    if (next.stage === SessionStage.Anonymous) {
      logoutBarrierRef.current = false;
      logoutCsrfRef.current = null;
    }
    installAuth(next);
    return next;
  }, [installAuth]);

  const refresh = useCallback(async (): Promise<AuthState | undefined> => {
    const generation = ++generationRef.current;
    try {
      return await readState(generation);
    } catch (failure) {
      if (generation !== generationRef.current || !mountedRef.current || isAbortError(failure)) return undefined;
      failAuth(failure);
      return undefined;
    }
  }, [failAuth, readState]);

  const requestReauthentication = useCallback(() => {
    if (authRef.current?.stage === SessionStage.Authenticated) setReauthenticationOpen(true);
  }, []);

  const transition = useCallback(async <T = AuthState,>(
    path: string,
    payload?: unknown,
    method: 'POST' | 'PUT' | 'PATCH' | 'DELETE' = 'POST',
  ): Promise<T> => {
    const generation = ++generationRef.current;
    const finishHttpTransition = beginHttpAuthTransition();
    const previousCsrf = authRef.current?.csrf_token;
    const acknowledgeDestination = path === '/totp/confirm'
      ? safeDashboardPath(new URLSearchParams(window.location.search).get('next')) ?? '/dashboard/overview'
      : null;
    actionsInFlightRef.current += 1;
    try {
      const response = await sendMutateRequest(`${BASE_URL}/auth${path}`, payload, { method });
      const result: unknown = response.status === 204 ? undefined : await response.json();
      if (!mountedRef.current || generation !== generationRef.current) throw abortError();

      const next = isAuthState(result) ? result
        : typeof result === 'object' && result !== null && 'auth' in result && isAuthState(result.auth) ? result.auth
          : null;
      if (next) {
        logoutBarrierRef.current = false;
        logoutCsrfRef.current = null;
        installAuth(next);
      }
      if (path === '/recovery-codes/regenerate' || path === '/links/inspect' || (path.startsWith('/sessions/') && method === 'DELETE')) {
        await readState(generation);
        if (!mountedRef.current || generation !== generationRef.current) throw abortError();
      }
      if (path.startsWith('/sessions/') && method === 'DELETE') invalidatePrivate();
      if (typeof result === 'object' && result !== null && 'recovery_codes' in result) {
        const codes = (result as RecoveryCodesResponse).recovery_codes;
        recoveryDestinationRef.current = acknowledgeDestination;
        setRecoveryCodes([...codes]);
      }
      if ((path === '/mfa/verify' || path === '/password/reset') && typeof payload === 'object' && payload !== null && 'code_type' in payload && payload.code_type === 'recovery') {
        setRecoveryLogin(true);
      }
      if (path === '/totp/confirm') setRecoveryLogin(false);
      if (rotationPaths[path] || (previousCsrf && authRef.current?.csrf_token !== previousCsrf) || (path.startsWith('/sessions/') && method === 'DELETE')) {
        channelRef.current?.postMessage({ type: 'invalidate' });
      }
      return result as T;
    } catch (failure) {
      if (!mountedRef.current || generation !== generationRef.current || isAbortError(failure)) throw abortError();
      const actionError = toHttpError(failure);
      if (actionError.status === 0 || actionError.status >= 500) {
        failAuth(actionError);
      } else if (actionError.error === 'ReauthenticationRequired') {
        requestReauthentication();
      } else if (actionError.error === 'Unauthenticated' || actionError.error === 'TwoFactorRequired' || actionError.error === 'CsrfRejected' || actionError.error === 'AuthenticationStateChanged') {
        if (actionError.error === 'Unauthenticated' || actionError.error === 'TwoFactorRequired') {
          hideAuthority();
          setStatus('loading');
        }
        try {
          await readState(generation);
        } catch (stateFailure) {
          if (!mountedRef.current || generation !== generationRef.current || isAbortError(stateFailure)) throw abortError();
          failAuth(stateFailure);
        }
      }
      if (!mountedRef.current || generation !== generationRef.current) throw abortError();
      throw actionError;
    } finally {
      actionsInFlightRef.current -= 1;
      finishHttpTransition();
    }
  }, [failAuth, hideAuthority, installAuth, invalidatePrivate, readState, requestReauthentication]);

  const logout = useCallback(async (): Promise<void> => {
    const csrf = authRef.current?.csrf_token ?? logoutCsrfRef.current;
    const generation = ++generationRef.current;
    const finishHttpTransition = beginHttpAuthTransition();
    logoutBarrierRef.current = true;
    logoutCsrfRef.current = csrf;
    hideAuthority();
    setStatus('loading');
    setError(null);
    channelRef.current?.postMessage({ type: 'invalidate' });
    actionsInFlightRef.current += 1;
    try {
      await apiFetch(`${BASE_URL}/auth/logout`, {
        method: 'POST',
        headers: csrf ? { 'X-CSRF-Token': csrf } : undefined,
      });
      if (!mountedRef.current || generation !== generationRef.current) throw abortError();
      channelRef.current?.postMessage({ type: 'invalidate' });
      navigate('/login', { replace: true });
      await readState(generation);
    } catch (failure) {
      if (!mountedRef.current || generation !== generationRef.current || isAbortError(failure)) return;
      failAuth(failure);
      // Obtain a fresh token for an explicit retry, never replay the mutation.
      try {
        const next = await fetchAuthState();
        if (mountedRef.current && generation === generationRef.current) logoutCsrfRef.current = next.csrf_token;
      } catch {
        // The failure screen remains authoritative while the store is unavailable.
      }
    } finally {
      actionsInFlightRef.current -= 1;
      finishHttpTransition();
    }
  }, [failAuth, hideAuthority, navigate, readState]);

  const acknowledgeRecoveryCodes = useCallback(() => {
    setRecoveryCodes(null);
    const destination = recoveryDestinationRef.current;
    recoveryDestinationRef.current = null;
    if (destination) navigate(destination, { replace: true });
  }, [navigate]);

  useEffect(() => {
    mountedRef.current = true;
    const invalidateAndRefresh = () => {
      generationRef.current += 1;
      hideAuthority();
      setStatus('loading');
      setError(null);
      void refresh();
    };
    setHttpAuthBoundary({
      invalidate: invalidateAndRefresh,
      reauthenticate: requestReauthentication,
      refresh: () => { void refresh(); },
    });
    if (typeof BroadcastChannel !== 'undefined') {
      const channel = new BroadcastChannel('doughbox-auth');
      channelRef.current = channel;
      channel.onmessage = (event: MessageEvent<unknown>) => {
        if (typeof event.data === 'object' && event.data !== null && 'type' in event.data && event.data.type === 'invalidate') invalidateAndRefresh();
      };
    }
    const refreshOnFocus = () => {
      if (document.visibilityState === 'visible' && actionsInFlightRef.current === 0) void refresh();
    };
    window.addEventListener('focus', refreshOnFocus);
    document.addEventListener('visibilitychange', refreshOnFocus);
    void refresh();
    return () => {
      mountedRef.current = false;
      generationRef.current += 1;
      setHttpAuthBoundary(null);
      setHttpCsrfToken(null);
      invalidatePrivateRequests();
      channelRef.current?.close();
      channelRef.current = null;
      window.removeEventListener('focus', refreshOnFocus);
      document.removeEventListener('visibilitychange', refreshOnFocus);
    };
  }, [hideAuthority, refresh, requestReauthentication]);

  useEffect(() => {
    if (!auth || status !== 'ready') return;
    const remaining = new Date(String(auth.expires_at)).getTime() - Date.now();
    const timeout = window.setTimeout(() => {
      if (actionsInFlightRef.current === 0) void refresh();
    }, Math.max(0, remaining));
    return () => window.clearTimeout(timeout);
  }, [auth, refresh, status]);

  const submitReauthentication = async (input: ReauthRequest) => {
    await transition('/reauth', input);
    setReauthenticationOpen(false);
  };

  return (
    <AuthContext.Provider value={{
      auth,
      status,
      error,
      authenticationEpoch,
      refresh,
      transition,
      logout,
      requestReauthentication,
      recoveryLogin,
      dismissRecoveryLoginNotice: () => setRecoveryLogin(false),
      acknowledgeRecoveryCodes,
    }}>
      {recoveryCodes ? <RecoveryCodesScreen codes={recoveryCodes} onAcknowledge={acknowledgeRecoveryCodes} /> : children}
      <ReauthenticationDialog open={reauthenticationOpen} onOpenChange={setReauthenticationOpen} onSubmit={submitReauthentication} />
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) throw new Error('useAuth must be used within AuthProvider.');
  return context;
}

export default useAuth;
