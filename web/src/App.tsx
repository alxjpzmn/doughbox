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
  // needs to be called to handle authentication at app level
  useAuth();

  return (
    <SWRConfig>
      <main className="pb-safe-or-32  md:pb-safe-or-8">
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
