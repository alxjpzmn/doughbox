import "./index.css";

import { Route, Switch } from "wouter";
import { SWRConfig } from "swr";
import Login from "@/pages/Login";
import Portfolio from "@/pages/dashboard/Portfolio";
import Performance from "@/pages/dashboard/Performance";
import Timeline from "@/pages/dashboard/Timeline";
import Positions from "@/pages/dashboard/Positions";
import Taxation from "@/pages/dashboard/Taxation";
import useIsMobile from "@/hooks/useIsMobile";
import { Menu, MobileMenu } from "@/components/Menu";
import useAuth from "@/hooks/useAuth";



export default function App() {
  const isMobile = useIsMobile();
  const { loggedIn } = useAuth()



  return (
    <SWRConfig
      value={{
        onError: (error) => {
          console.log(loggedIn)
          console.error(error)
          return error;
        },
      }}
    >
      <main className="w-full min-h-screen pt-4 pb-20 px-4 bg-tremor-background-muted dark:bg-dark-tremor-background-muted">
        <div className="max-w-2xl mx-auto">
          <Switch>
            <Route path="/login" component={Login} />
            <Route path="/dashboard" nest>
              {isMobile ? <MobileMenu /> : <Menu />}
              <Route path="/portfolio" component={Portfolio} />
              <Route path="/performance" component={Performance} />
              <Route path="/timeline" component={Timeline} />
              <Route path="/positions" component={Positions} />
              <Route path="/taxation" component={Taxation} />
            </Route>
          </Switch>
        </div >
      </main >
    </SWRConfig >
  );
}
