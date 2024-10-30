import "./index.css";

import useAuth from "./hooks/useAuth";
import { useEffect } from "react";
import { Route, Switch, useLocation } from "wouter";
import { SWRConfig } from "swr";
import Dashboard from "./pages/dashboard/Dashboard";
import Login from "./pages/Login";


export default function App() {
  const { loggedIn, loading, redirect } = useAuth();
  const [location] = useLocation();

  useEffect(() => {
    redirect();
  }, [loggedIn, location, loading]);

  return (
    <SWRConfig
      value={{
        onError: (error) => {
          console.log(error)
          return error;
        },
      }}
    >
      <main className="w-full min-h-screen pt-4 pb-20 px-4 bg-tremor-background-muted dark:bg-dark-tremor-background-muted">
        <div className="max-w-2xl mx-auto">
          <Switch>
            <Route path="/login" component={Login} />
            <Route path="/dashboard" component={Dashboard} />
          </Switch>
        </div>
      </main>
    </SWRConfig>
  );
}
