import "./index.css";

import { Route, Switch } from "wouter";
import { SWRConfig } from "swr";
import Login from "@/pages/Login";
import Positions from "@/pages/dashboard/Positions";
import PL from "@/pages/dashboard/PL";
import Timeline from "@/pages/dashboard/Timeline";
import Dividends from "@/pages/dashboard/Dividends";
import ActiveUnits from "@/pages/dashboard/ActiveUnits";
import Taxation from "@/pages/dashboard/Taxation";
import useIsMobile from "@/hooks/useIsMobile";
import { Menu, MobileMenu } from "@/components/Menu";



export default function App() {
  const isMobile = useIsMobile();

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
            <Route path="/dashboard" nest>
              {isMobile ? <MobileMenu /> : <Menu />}
              <Route path="/positions" component={Positions} />
              <Route path="/pl" component={PL} />
              <Route path="/timeline" component={Timeline} />
              <Route path="/dividends" component={Dividends} />
              <Route path="/units" component={ActiveUnits} />
              <Route path="/taxation" component={Taxation} />
            </Route>
          </Switch>
        </div >
      </main >
    </SWRConfig >
  );
}
