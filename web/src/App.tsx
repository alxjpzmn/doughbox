import "./index.css";

import useAuth from "@/hooks/useAuth";
import { Link, Route, Switch, useLocation } from "wouter";
import { SWRConfig } from "swr";
import Login from "@/pages/Login";
import Positions from "@/pages/dashboard/Positions";
import PL from "@/pages/dashboard/PL";
import Timeline from "@/pages/dashboard/Timeline";
import Dividends from "@/pages/dashboard/Dividends";
import ActiveUnits from "@/pages/dashboard/ActiveUnits";
import Taxation from "@/pages/dashboard/Taxation";
import { TabNavigation, TabNavigationLink } from "@/components/TabNavigation";


export default function App() {
  const { logout } = useAuth();
  const [location] = useLocation();

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
              <TabNavigation className="mb-10 flex items-center justify-between">
                <div className="flex">
                  <TabNavigationLink asChild active={location === '/dashboard/positions'}>
                    <Link href="/positions">Portfolio</Link>
                  </TabNavigationLink>
                  <TabNavigationLink asChild active={location === '/dashboard/pl'}>
                    <Link href="/pl">P&L</Link>
                  </TabNavigationLink>
                  <TabNavigationLink asChild active={location === '/dashboard/timeline'}>
                    <Link href="/timeline">Timeline</Link>
                  </TabNavigationLink>
                  <TabNavigationLink asChild active={location === '/dashboard/dividends'}>
                    <Link href="/dividends">Dividends</Link>
                  </TabNavigationLink>
                  <TabNavigationLink asChild active={location === '/dashboard/units'}>
                    <Link href="/units">Units</Link>
                  </TabNavigationLink>
                  <TabNavigationLink asChild active={location === '/dashboard/taxation'}>
                    <Link href="/taxation">Taxation</Link>
                  </TabNavigationLink>
                </div>
                <button className="text-red-400 text-sm pb-2" onClick={async () => {
                  await logout()
                }} type='button'>Logout</button>
              </TabNavigation>
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
