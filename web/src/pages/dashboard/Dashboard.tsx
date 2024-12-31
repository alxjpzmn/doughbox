import { Tab, TabList, TabGroup, TabPanels, TabPanel } from "@tremor/react";
import useAuth from "@/hooks/useAuth";
import Performance from "@/pages/dashboard/Performance";
import Taxation from "@/pages/dashboard/Taxation";
import Timeline from "@/pages/dashboard/Timeline";
import Positions from "@/pages/dashboard/Positions";
import Portfolio from "@/pages/dashboard/Portfolio";

const Dashboard = () => {
  const { logout } = useAuth();

  return (
    <div>
      <TabGroup>
        <TabList className="mb-6 flex justify-between items-center overflow-x-auto overflow-y-hidden flex-shrink-0">
          <div className="flex">
            <Tab className="flex-shrink-0" value={1}>
              Portfolio
            </Tab>
            <Tab className="flex-shrink-0" value={2}>
              Performance
            </Tab>
            <Tab className="flex-shrink-0" value={3}>
              Timeline
            </Tab>
            <Tab className="flex-shrink-0" value={4}>Positions</Tab>
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
            <Portfolio />
          </TabPanel>
          <TabPanel>
            <Performance />
          </TabPanel>
          <TabPanel>
            <Timeline />
          </TabPanel>
          <TabPanel>
            <Positions />
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
