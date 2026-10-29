import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import Portfolio from "@/pages/dashboard/portfolio";
import Timeline from "@/pages/dashboard/timeline";
import Positions from "@/pages/dashboard/positions";
import Assets from "@/pages/dashboard/assets";

const Overview = () => (
  <Tabs defaultValue="portfolio">
    <TabsList className="mb-6 grid w-full grid-cols-3 sm:inline-flex sm:w-fit" aria-label="Portfolio sections">
      <TabsTrigger value="portfolio">Summary</TabsTrigger>
      <TabsTrigger value="holdings">Holdings</TabsTrigger>
      <TabsTrigger value="timeline">Timeline</TabsTrigger>
    </TabsList>

    <TabsContent value="portfolio" forceMount className="data-[state=inactive]:hidden">
      <Portfolio />
    </TabsContent>
    <TabsContent value="holdings" forceMount className="data-[state=inactive]:hidden">
      <Tabs defaultValue="imported">
        <TabsList className="mb-6" aria-label="Holdings sections">
          <TabsTrigger value="imported">Imported</TabsTrigger>
          <TabsTrigger value="manual">Manual</TabsTrigger>
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

export default Overview;
