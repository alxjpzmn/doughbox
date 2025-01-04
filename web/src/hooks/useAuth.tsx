import { useEffect } from "react";
import useSWR, { useSWRConfig } from "swr";
import { useLocation, useRouter } from "wouter";
import { BASE_URL, clearSWRCache, fetcher, sendMutateRequest } from "@/util";
import { useBrowserLocation } from "wouter/use-browser-location";

const useAuth = () => {
  const { cache } = useSWRConfig();
  const router = useRouter();
  const [location, navigate] = useLocation();
  const [_browserLocation, setBrowserlocation] = useBrowserLocation();

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


  const routeRequiresAuth = router.base === "/dashboard";
  const loading = isLoading;
  const loggedOut = !!error && error.status === 401;
  const loggedIn = !loggedOut;

  if (!isLoading && loggedIn && (location === "/login" || location === '/')) {
    navigate("/dashboard/portfolio");
  }

  if (!isLoading && loggedOut && (routeRequiresAuth || location === '/')) {
    setBrowserlocation("/login", { replace: true });
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
        navigate("/dashboard/portfolio");
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
