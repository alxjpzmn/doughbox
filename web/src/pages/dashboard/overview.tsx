import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import Portfolio from "@/pages/dashboard/portfolio";
import Timeline from "@/pages/dashboard/timeline";
import Positions from "@/pages/dashboard/positions";

const Overview = () => (
  <Tabs defaultValue="portfolio">
    <TabsList className="mb-6" aria-label="Overview sections">
      <TabsTrigger value="portfolio">Portfolio</TabsTrigger>
      <TabsTrigger value="timeline">Timeline</TabsTrigger>
      <TabsTrigger value="positions">Positions</TabsTrigger>
    </TabsList>

    <TabsContent value="portfolio" forceMount className="data-[state=inactive]:hidden">
      <Portfolio />
    </TabsContent>
    <TabsContent value="timeline" forceMount className="data-[state=inactive]:hidden">
      <Timeline />
    </TabsContent>
    <TabsContent value="positions" forceMount className="data-[state=inactive]:hidden">
      <Positions />
    </TabsContent>
  </Tabs>
);

export default Overview;
