import useSWR from "swr";
import { useState } from "react";
import { format } from "date-fns";
import { DividendCard, FxCard, InterestCard, SkeletonCard, TradeCard } from "@/components/composite/event-card";
import { EventType, PortfolioEvent } from "@/types/core";
import EmptyState from "@/components/composite/empty-state";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Calendar } from "@/components/ui/calendar";
import { CalendarIcon } from "lucide-react";
import { BASE_URL, fetcher } from "@/lib/http";

export const Timeline = () => {
  const [selectedDate, setSelectedDate] = useState<Date>(new Date(new Date().setMonth(new Date().getMonth() - 3)));
  const { data, isLoading } = useSWR<PortfolioEvent[]>(
    `${BASE_URL}/timeline?start_date=${format(selectedDate, "yyyy-LL-dd")}`,
    fetcher,
  );

  return (
    <>
      <Card className="w-full flex flex-col justify-start mb-6">
        <CardHeader>
          <CardTitle>
            Start Date
          </CardTitle>
        </CardHeader>
        <CardContent>
          <Popover>
            <PopoverTrigger asChild>
              <Button
                variant={"outline"}
                className={cn(
                  "w-[240px] justify-start text-left font-normal",
                  !selectedDate && "text-muted-foreground"
                )}
              >
                <CalendarIcon />
                {selectedDate ? format(selectedDate, "PPP") : <span>Pick a date</span>}
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-auto p-0" align="start">
              <Calendar
                mode="single"
                selected={selectedDate}
                // @ts-ignore
                onSelect={setSelectedDate}
                initialFocus
              />
            </PopoverContent>
          </Popover>
        </CardContent>
      </Card>

      {/* Skeleton for Timeline Events */}
      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {Array.from({ length: 100 }).map((_, index) => (
            <SkeletonCard key={index} />
          ))}
        </div>
      ) : data?.length === 0 ? (
        <EmptyState />
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {data?.map((timelineEvent: PortfolioEvent) => {
            let identifier = `${timelineEvent.date}${timelineEvent.identifier}${timelineEvent.units}`;
            let eventComponent;
            switch (timelineEvent.event_type) {
              case EventType.Trade:
                eventComponent = <TradeCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              case EventType.CashInterest:
                eventComponent = <InterestCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              case EventType.ShareInterest:
                eventComponent = <InterestCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              case EventType.Dividend:
                eventComponent = <DividendCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              case EventType.DividendAequivalent:
                eventComponent = <DividendCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              case EventType.FxConversion:
                eventComponent = <FxCard timelineEvent={timelineEvent} key={identifier} />;
                break;
              default:
                eventComponent = null;
            }
            return eventComponent;
          })}
        </div >
      )}
      <Disclaimer />
    </>
  );
};

export default Timeline;
