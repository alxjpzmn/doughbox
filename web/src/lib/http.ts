import { AuthState } from "@/types/core";

export const BASE_URL = '/api';

export interface HttpError {
  status: number;
  error: string;
  message: string;
  details?: unknown;
  retryAfter?: string;
}

interface MutateRequestOptions {
  method: 'POST' | 'PUT' | 'PATCH' | 'DELETE';
}

interface AuthBoundaryHandlers {
  invalidate: () => void;
  reauthenticate: () => void;
  refresh: () => void;
}

type FetcherArgs = [string, RequestInit?];
let csrfToken: string | null = null;
let privateRequestEpoch = 0;
let credentialEpoch = 0;
let authTransitionsInFlight = 0;
let pendingAuthState: { epoch: number; promise: Promise<AuthState> } | null = null;
let boundaryHandlers: AuthBoundaryHandlers | null = null;
const privateRequests = new Set<AbortController>();

export const getPrivateRequestEpoch = (): number => privateRequestEpoch;
export const isCurrentPrivateRequestEpoch = (epoch: number): boolean => epoch === privateRequestEpoch;

export function invalidatePrivateRequests(): number {
  privateRequestEpoch += 1;
  for (const controller of privateRequests) controller.abort();
  privateRequests.clear();
  return privateRequestEpoch;
}

export function setHttpCsrfToken(token: string | null): void {
  if (csrfToken !== token) credentialEpoch += 1;
  csrfToken = token;
}

/** Suspend old-credential error effects without discarding same-user financial data. */
export function beginHttpAuthTransition(): () => void {
  credentialEpoch += 1;
  authTransitionsInFlight += 1;
  return () => {
    credentialEpoch += 1;
    authTransitionsInFlight -= 1;
  };
}

export function setHttpAuthBoundary(handlers: AuthBoundaryHandlers | null): void {
  boundaryHandlers = handlers;
}

export function abortError(): DOMException {
  return new DOMException('This request belongs to an obsolete authentication state.', 'AbortError');
}

export function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}

export function toHttpError(error: unknown): HttpError {
  if (typeof error === 'object' && error !== null && 'status' in error && 'error' in error && 'message' in error) {
    return error as HttpError;
  }
  return {
    status: 0,
    error: 'NetworkError',
    message: 'Unable to reach Doughbox. Check your connection and try again.',
  };
}

async function responseError(response: Response): Promise<HttpError> {
  let body: unknown;
  try {
    if (response.headers.get('content-type')?.includes('application/json')) body = await response.json();
  } catch (error) {
    if (isAbortError(error)) throw error;
  }
  const error = toHttpError(body);
  return {
    ...error,
    status: response.status,
    error: error.status === 0 ? 'UnknownError' : error.error,
    message: error.status === 0 ? 'The request could not be completed. Please try again.' : error.message,
    retryAfter: response.headers.get('Retry-After') ?? undefined,
  };
}

function handlePrivateAuthError(error: HttpError, epoch: number): void {
  const handler = error.error === 'Unauthenticated' || error.error === 'TwoFactorRequired'
    ? boundaryHandlers?.invalidate
    : error.error === 'ReauthenticationRequired'
      ? boundaryHandlers?.reauthenticate
      : error.error === 'CsrfRejected' || error.error === 'AuthenticationStateChanged'
        ? boundaryHandlers?.refresh
        : undefined;
  if (!handler) return;
  if (epoch !== credentialEpoch || authTransitionsInFlight > 0) throw abortError();
  handler();
}

/** Raw successful response, with the same no-store/CSRF policy as SWR requests. */
export async function apiFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const request = input instanceof Request ? input : undefined;
  const url = new URL(request?.url ?? String(input), window.location.origin);
  const sameOrigin = url.origin === window.location.origin;
  const isApi = sameOrigin && (url.pathname === BASE_URL || url.pathname.startsWith(`${BASE_URL}/`));
  const isAuth = isApi && (url.pathname === `${BASE_URL}/auth` || url.pathname.startsWith(`${BASE_URL}/auth/`));
  const isPrivate = isApi && !isAuth;
  const epoch = privateRequestEpoch;
  const sourceCredentialEpoch = credentialEpoch;
  const method = (init.method ?? request?.method ?? 'GET').toUpperCase();
  const unsafe = !['GET', 'HEAD', 'OPTIONS'].includes(method);
  const headers = new Headers(init.headers ?? request?.headers);
  if (isApi && unsafe && csrfToken && !headers.has('X-CSRF-Token')) headers.set('X-CSRF-Token', csrfToken);

  // A cross-tab invalidation removes the token before React can unmount the old tree.
  // Do not let a click in that interval submit a request with stale authority.
  // With no valid server session, explicit same-origin logout may only clear a cookie.
  // The server still requires matching CSRF whenever that cookie resolves to a session.
  const logout = isAuth && url.pathname === `${BASE_URL}/auth/logout` && method === 'POST';
  if (isApi && unsafe && !csrfToken && !headers.has('X-CSRF-Token') && !logout) throw abortError();

  const controller = new AbortController();
  const sourceSignal = init.signal ?? request?.signal;
  const abort = () => controller.abort();
  if (sourceSignal?.aborted) controller.abort();
  else sourceSignal?.addEventListener('abort', abort, { once: true });
  if (isPrivate) privateRequests.add(controller);

  try {
    const response = await fetch(input, {
      ...init,
      headers,
      credentials: init.credentials ?? (sameOrigin ? 'same-origin' : 'omit'),
      cache: isApi ? 'no-store' : init.cache,
      signal: controller.signal,
    });
    if (isPrivate && !isCurrentPrivateRequestEpoch(epoch)) throw abortError();
    if (!response.ok) {
      const error = await responseError(response);
      if (isPrivate && !isCurrentPrivateRequestEpoch(epoch)) throw abortError();
      // Auth actions have their own generation guard in AuthProvider.
      if (isPrivate) handlePrivateAuthError(error, sourceCredentialEpoch);
      throw error;
    }
    return response;
  } catch (error) {
    if (isAbortError(error) || (isPrivate && !isCurrentPrivateRequestEpoch(epoch))) throw abortError();
    throw toHttpError(error);
  } finally {
    privateRequests.delete(controller);
    sourceSignal?.removeEventListener('abort', abort);
  }
}

export const fetcher = async (...[input, init]: FetcherArgs) => {
  const epoch = getPrivateRequestEpoch();
  const url = new URL(input, window.location.origin);
  const isPrivate = url.origin === window.location.origin && url.pathname.startsWith(`${BASE_URL}/`) && !url.pathname.startsWith(`${BASE_URL}/auth/`);
  try {
    const response = await apiFetch(input, init);
    const result = response.headers.get('content-type')?.includes('application/json')
      ? await response.json()
      : await response.text();
    if (isPrivate && !isCurrentPrivateRequestEpoch(epoch)) throw abortError();
    return result;
  } catch (error) {
    if (isAbortError(error) || (isPrivate && !isCurrentPrivateRequestEpoch(epoch))) throw abortError();
    throw toHttpError(error);
  }
};

/** Share cookie-setting initial reads, including StrictMode effect replay. */
export function fetchAuthState(): Promise<AuthState> {
  if (pendingAuthState?.epoch === credentialEpoch) return pendingAuthState.promise;
  const request = { epoch: credentialEpoch, promise: fetcher(`${BASE_URL}/auth/state`) as Promise<AuthState> };
  pendingAuthState = request;
  const clear = () => { if (pendingAuthState === request) pendingAuthState = null; };
  void request.promise.then(clear, clear);
  return request.promise;
}

export const sendMutateRequest = async (
  apiPath: string,
  payload?: unknown,
  requestOptions: MutateRequestOptions = { method: 'POST' },
): Promise<Response> => apiFetch(apiPath, {
  method: requestOptions.method,
  headers: payload instanceof FormData ? undefined : { 'Content-Type': 'application/json' },
  body: payload === undefined ? undefined : payload instanceof FormData ? payload : JSON.stringify(payload),
});
