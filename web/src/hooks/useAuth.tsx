import { BASE_URL, fetcher, clearSWRCache, sendMutateRequest } from "@/lib/http";
import { useEffect } from "react";
import useSWR, { useSWRConfig } from "swr";
import { useLocation } from "wouter";
import { useBrowserLocation } from "wouter/use-browser-location";

const proposalIdPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function focusedProposalId(search = window.location.search): string | null {
  const id = new URLSearchParams(search).get("proposal");
  return id && proposalIdPattern.test(id) ? id : null;
}

function safeDashboardPath(value: string | null): string | null {
  if (!value || !value.startsWith("/dashboard/")) return null;
  if (value.includes("\\") || value.includes("://") || value.includes("..") || value.includes("//") || value.includes("\0")) return null;
  return value;
}

function approvalDestination(): string | null {
  const params = new URLSearchParams(window.location.search);
  const proposal = focusedProposalId();
  return safeDashboardPath(params.get("next"))
    ?? (proposal ? `/dashboard/trading?proposal=${encodeURIComponent(proposal)}` : null)
    ?? safeDashboardPath(`${window.location.pathname}${window.location.search}`);
}

function loginLocation(): string {
  const destination = approvalDestination();
  return destination ? `/login?next=${encodeURIComponent(destination)}` : "/login";
}

const useAuth = () => {
  const { cache } = useSWRConfig();
  const [location, navigate] = useLocation();
  const [browserLocation, setBrowserlocation] = useBrowserLocation();

  const { isLoading, mutate, error } = useSWR(
    `${BASE_URL}/auth_state`,
    fetcher,
    {
      onErrorRetry: (error) => {
        // Never retry on 401
        if (error.status === 401) {
          return
        };
      },
    },
  );


  // The app-level hook runs outside the nested dashboard router on mobile.
  const routeRequiresAuth = browserLocation === "/dashboard" || browserLocation.startsWith("/dashboard/");
  const loading = isLoading;
  const loggedOut = !!error && error.status === 401;
  const loggedIn = !loggedOut;

  if (!isLoading && loggedIn && (location === "/login" || location === '/')) {
    navigate(approvalDestination() ?? "/dashboard/overview");
  }

  if (!isLoading && loggedOut && (routeRequiresAuth || location === '/')) {
    setBrowserlocation(loginLocation(), { replace: true });
  }

  useEffect(() => {
    if (loggedOut) {
      clearSWRCache(cache);
    }
  }, []);

  const logout = async () => {
    const res = await sendMutateRequest(`${BASE_URL}/logout`);
    if (res.ok) {
      mutate();
      clearSWRCache(cache);
      navigate("/login", { replace: true });
    }
  };

  const login = async (password: string) => {
    try {
      const res = await sendMutateRequest(`${BASE_URL}/login`, { password });
      if (!res.ok) {
        const errorData = await res.json();
        throw new Error(errorData.message || "Login failed");
      } else {
        mutate();
        clearSWRCache(cache);
        navigate(approvalDestination() ?? "/dashboard/overview");
      }
    } catch (err) {
      throw err;
    }
  };


  return {
    loading,
    loggedIn,
    error,
    logout,
    login,
    mutate,
  };
};

export default useAuth;
