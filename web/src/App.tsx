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
import { Text } from "@tremor/react";
import { Callout } from "./components/Callout";



export default function App() {
  const isMobile = useIsMobile();
  // needs to be called to handle authentication at app level
  useAuth();

  return (
    <SWRConfig>
      <main className="w-full min-h-screen pt-4 pb-20 px-4 bg-tremor-background-muted dark:bg-dark-tremor-background-muted h-screen overflow-y-auto scrollbar-gutter-stable">
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
          <Text className="mb-6 py-6">
          </Text>
          <Callout variant="default" title="Disclaimer">
            Doughbox is <a href="https://github.com/alxjpzmn/doughbox" target="_blank" className="underline">open source software</a> under the <a href="https://github.com/alxjpzmn/doughbox/blob/main/LICENSE" className="underline" target="_blank">MIT license</a>. It comes without any warranty or responsibility taken for the accuracy or completeness of the data shown.
          </Callout>
        </div >
      </main >
    </SWRConfig >
  );
}
