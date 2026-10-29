import "./index.css";
import { Route, Switch } from "wouter";
import { SWRConfig } from "swr";
import Login from "@/pages/login";
import Overview from "@/pages/dashboard/overview";
import Performance from "@/pages/dashboard/performance";
import Taxation from "@/pages/dashboard/taxation";
import Instruments from "@/pages/dashboard/instruments";
import useIsMobile from "@/hooks/useIsMobile";
import { Menu, MobileMenu } from "@/components/composite/menu";
import useAuth from "@/hooks/useAuth";
import { ThemeProvider } from "@/components/theme-provider"
import { ApplicationSettingsPanel } from "./components/composite/application-settings";




export default function App() {
  const isMobile = useIsMobile();
  // needs to be called to handle authentication at app level
  useAuth();

  return (
    <ThemeProvider storageKey="vite-ui-theme">
      <SWRConfig>
        <Switch>
          <Route path="/login">
            <main className="px-4 py-2 pb-safe-or-32 md:pb-safe-or-8">
              <div className="max-w-2xl mx-auto">
                <Login />
              </div>
            </main>
          </Route>
          <Route path="/dashboard" nest>
            <div className="fixed inset-0 flex flex-col md:static md:block">
              <main className="min-h-0 flex-1 overflow-y-auto p-6 md:overflow-visible md:px-4 md:py-2 md:pb-safe-or-8">
                <div className="max-w-2xl mx-auto">
                  {!isMobile && <Menu />}
                  <Route path="/overview" component={Overview} />
                  <Route path="/performance" component={Performance} />
                  <Route path="/instruments" component={Instruments} />
                  <Route path="/taxation" component={Taxation} />
                  <Route path="/settings" component={ApplicationSettingsPanel} />
                </div>
              </main>
              {isMobile && <MobileMenu />}
            </div>
          </Route>
        </Switch>
      </SWRConfig >
    </ThemeProvider>
  );
}
