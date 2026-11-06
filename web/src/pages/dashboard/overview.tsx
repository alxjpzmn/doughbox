import { useLocation } from "wouter";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import Portfolio from "@/pages/dashboard/portfolio";
import Performance from "@/pages/dashboard/performance";
import Positions from "@/pages/dashboard/positions";
import Timeline from "@/pages/dashboard/timeline";
import Assets from "@/pages/dashboard/assets";

const Overview = () => {
  const [location] = useLocation();
  return (
  <Tabs defaultValue={location.endsWith("/performance") ? "performance" : "portfolio"} className="max-md:pb-52">
    <TabsList dock className="mb-6 h-auto sm:inline-flex sm:w-fit" aria-label="Portfolio sections">
      <TabsTrigger value="portfolio" className="min-h-11">Summary</TabsTrigger>
      <TabsTrigger value="performance" className="min-h-11">Performance</TabsTrigger>
      <TabsTrigger value="holdings" className="min-h-11">Holdings</TabsTrigger>
      <TabsTrigger value="timeline" className="min-h-11">Timeline</TabsTrigger>
    </TabsList>

    <TabsContent value="portfolio" forceMount className="data-[state=inactive]:hidden">
      <Portfolio />
    </TabsContent>
    <TabsContent value="performance" forceMount className="data-[state=inactive]:hidden">
      <Performance />
    </TabsContent>
    <TabsContent value="holdings" forceMount className="data-[state=inactive]:hidden">
      <Tabs defaultValue="imported">
        <TabsList dock="stacked" className="mb-6 h-auto sm:w-fit" aria-label="Holdings sections">
          <TabsTrigger value="imported" className="min-h-11">Imported</TabsTrigger>
          <TabsTrigger value="manual" className="min-h-11">Manual</TabsTrigger>
        </TabsList>
        <TabsContent value="imported" forceMount className="data-[state=inactive]:hidden">
          <Positions />
        </TabsContent>
        <TabsContent value="manual" forceMount className="data-[state=inactive]:hidden">
          <Assets />
        </TabsContent>
      </Tabs>
    </TabsContent>
    <TabsContent value="timeline" forceMount className="data-[state=inactive]:hidden">
      <Timeline />
    </TabsContent>
  </Tabs>
  );
};

export default Overview;
