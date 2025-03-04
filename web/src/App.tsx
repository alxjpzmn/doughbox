import "./index.css";
import { Route, Switch } from "wouter";
import { SWRConfig } from "swr";
import Login from "@/pages/login";
import Portfolio from "@/pages/dashboard/portfolio";
import Performance from "@/pages/dashboard/performance";
import Timeline from "@/pages/dashboard/timeline";
import Positions from "@/pages/dashboard/positions";
import Taxation from "@/pages/dashboard/taxation";
import useIsMobile from "@/hooks/useIsMobile";
import { Menu, MobileMenu } from "@/components/composite/menu";
import useAuth from "@/hooks/useAuth";
import { ThemeProvider } from "@/components/theme-provider"




export default function App() {
  const isMobile = useIsMobile();
  // needs to be called to handle authentication at app level
  useAuth();

  return (
    <ThemeProvider storageKey="vite-ui-theme">
      <SWRConfig>
        <main className="px-4 py-2 pb-safe-or-32 md:pb-safe-or-8">
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
    </ThemeProvider>
  );
}
