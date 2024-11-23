import { Tab, TabList, TabGroup, TabPanels, TabPanel } from "@tremor/react";
import useAuth from "@/hooks/useAuth";
import Positions from "@/pages/dashboard/Positions";
import PL from "@/pages/dashboard/PL";
import Dividends from "@/pages/dashboard/Dividends";
import ActiveUnits from "@/pages/dashboard/ActiveUnits";
import Taxation from "@/pages/dashboard/Taxation";
import Timeline from "@/pages/dashboard/Timeline";

const Dashboard = () => {
  const { logout } = useAuth();

  return (
    <div>
      <TabGroup>
        <TabList className="mb-6 flex justify-between items-center overflow-x-auto overflow-y-hidden flex-shrink-0">
          <div className="flex">
            <Tab className="flex-shrink-0" value={1}>
              Positions
            </Tab>
            <Tab className="flex-shrink-0" value={2}>
              Profit & Loss
            </Tab>
            <Tab className="flex-shrink-0" value={2}>
              Timeline
            </Tab>
            <Tab className="flex-shrink-0" value={3}>
              Dividends
            </Tab>
            <Tab className="flex-shrink-0" value={4}>
              Active Units
            </Tab>
            <Tab className="flex-shrink-0" value={5}>
              Taxation
            </Tab>
          </div>
          <button className="text-red-400 text-sm" onClick={async () => {
            await logout()
          }} type='button'>Logout</button>
        </TabList>
        <TabPanels>
          <TabPanel>
            <Positions />
          </TabPanel>
          <TabPanel>
            <PL />
          </TabPanel>
          <TabPanel>
            <Timeline />
          </TabPanel>
          <TabPanel>
            <Dividends />
          </TabPanel>
          <TabPanel>
            <ActiveUnits />
          </TabPanel>
          <TabPanel>
            <Taxation />
          </TabPanel>
        </TabPanels>
      </TabGroup>
    </div>
  )
}

export default Dashboard;
