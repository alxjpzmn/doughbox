import { BASE_URL, fetcher } from "@/util";
import useSwr from "swr";
import { Card, Title, List, ListItem } from "@tremor/react";
import { useState } from "react";
import { DatePicker } from "@tremor/react";
import { format } from "date-fns";

const Positions = ({ }) => {
  const [selectedDate, setSelectedDate] = useState(new Date());
  const { data } = useSwr(
    `${BASE_URL}/positions?date=${format(selectedDate, "yyyy-LL-dd")}`,
    fetcher,
  );

  return (
    <div className="min-h-screen">
      <Card className="w-full flex flex-col justify-start mb-6">
        <Title className="w-full mb-4">Positions</Title>
        <DatePicker
          enableYearNavigation
          className="max-w-sm"
          // @ts-ignore
          onValueChange={(value: string) => setSelectedDate(new Date(value))}
          enableClear={false}
        />
      </Card>
      {data?.length > 0 && (
        <Card>
          <List>
            {data?.map((item: any) => (
              <ListItem key={`${item?.isin}`}>
                <a href={`https://duckduckgo.com/?q=${item?.isin}`}>
                  {item?.isin}
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
