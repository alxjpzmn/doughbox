import useSWR from "swr";
import { useState } from "react";
import { format } from "date-fns";
import { Card, DatePicker, Title } from "@tremor/react";
import { BASE_URL, fetcher } from "@/util";
import { DividendCard, FxCard, InterestCard, TradeCard } from "@/components/EventCard";
import { EventType, PortfolioEvent } from "@/types/core";

export const Timeline = () => {


  const [selectedDate, setSelectedDate] = useState<Date>(new Date(new Date().setMonth(new Date().getMonth() - 3)));
  const { data } = useSWR<PortfolioEvent[]>(
    `${BASE_URL}/timeline?start_date=${format(selectedDate, "yyyy-LL-dd")}`,
    fetcher,
  );

  return (
    <div>
      <Card className="w-full flex flex-col justify-start mb-6">
        <Title className="w-full mb-4">Start Date</Title>
        <DatePicker
          enableYearNavigation
          className="max-w-sm"
          value={selectedDate}
          // @ts-ignore
          onValueChange={(value) => setSelectedDate(new Date(value))}
          enableClear={false}
        />
      </Card>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {data?.
          map((timelineEvent: PortfolioEvent) => {
            let eventComponent;
            switch (timelineEvent.event_type) {
              case EventType.Trade:
                eventComponent = <TradeCard timelineEvent={timelineEvent} />;
                break;
              case EventType.CashInterest:
                eventComponent = <InterestCard timelineEvent={timelineEvent} />;
                break;
              case EventType.ShareInterest:
                eventComponent = <InterestCard timelineEvent={timelineEvent} />;
                break;
              case EventType.Dividend:
                eventComponent = <DividendCard timelineEvent={timelineEvent} />;
                break;
              case EventType.DividendAequivalent:
                eventComponent = <DividendCard timelineEvent={timelineEvent} />;
                break;
              case EventType.FxConversion:
                eventComponent = <FxCard timelineEvent={timelineEvent} />;
                break;
              default:
                eventComponent = null;
            }
            return eventComponent;

          }
          )}
      </div>
    </div>
  )
}

export default Timeline;
