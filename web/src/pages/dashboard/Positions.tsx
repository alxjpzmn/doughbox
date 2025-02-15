import { BASE_URL, fetcher } from "@/util";
import useSwr from "swr";
import { Card, Title, List, ListItem } from "@tremor/react";
import { useState } from "react";
import { DatePicker } from "@tremor/react";
import { format } from "date-fns";
import { PositionWithName } from "@/types/core";
import EmptyState from "@/components/EmptyState";
import { Skeleton } from "@/components/Skeleton";

const Positions = () => {
  const [selectedDate, setSelectedDate] = useState(new Date());
  const { data, isLoading } = useSwr<PositionWithName[]>(
    `${BASE_URL}/positions?date=${format(selectedDate, "yyyy-LL-dd")}`,
    fetcher,
  );


  return (
    <div>
      <Card className="w-full flex flex-col justify-start mb-6">
        <Title className="w-full mb-4">Date</Title>
        <DatePicker
          enableYearNavigation
          className="max-w-sm"
          // @ts-ignore
          onValueChange={(value: string) => setSelectedDate(new Date(value))}
          enableClear={false}
          defaultValue={new Date()}
        />
      </Card>
      {isLoading ? (
        <Card>
          {Array.from({ length: 100 }).map((_, index) => (
            <div key={index} className='flex justify-between gap-6 p-2'>
              <Skeleton className="h-6 w-3/4" />
              <Skeleton className="h-6 w-1/4" />
            </div>
          ))}
        </Card>
      ) : data?.length === 0 ? <EmptyState /> : (
        <Card>
          <List>
            {data?.map((item) => (
              <ListItem key={`${item?.isin}`}>
                <a href={`https://duckduckgo.com/?q=${item?.isin}`} className="truncate overflow-hidden whitespace-nowrap max-w-48">
                  {item?.name}
                </a>
                <span className="font-bold">
                  {item?.units}
                </span>
              </ListItem>
            ))}
          </List>
        </Card>
      )}
    </div>
  );
};


export default Positions;
