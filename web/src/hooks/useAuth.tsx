import { useEffect } from "react";
import useSWR, { useSWRConfig } from "swr";
import { useLocation } from "wouter";
import { BASE_URL, clearSWRCache, fetcher, sendMutateRequest } from "@/util";
import { navigate } from "wouter/use-browser-location";

const useAuth = () => {
  const { cache } = useSWRConfig();
  const [location, setLocation] = useLocation();

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

  const loading = isLoading;
  const loggedOut = !!error && error.status === 401;
  const loggedIn = !loggedOut;

  if (!isLoading && loggedIn && (location === "/login" || location === '/')) {
    setLocation("/dashboard/portfolio");
  }

  if (!isLoading && loggedOut && (location.includes("/dashboard") || location === '/')) {
    setLocation("/login");
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
        setLocation("/dashboard/portfolio");
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
