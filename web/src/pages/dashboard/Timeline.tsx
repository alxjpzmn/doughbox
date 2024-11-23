import useSWR from "swr";
import { useState } from "react";
import { format } from "date-fns";
import { Card, DatePicker, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow, Text, Title } from "@tremor/react";
import { BASE_URL, fetcher, formatCurrency, formatDate } from "@/util";

export const Timeline = () => {


  const [selectedDate, setSelectedDate] = useState<Date>(new Date(new Date().setMonth(new Date().getMonth() - 3)));
  const { data, isLoading: loading } = useSWR(
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
      {loading ? <Text>Loading...</Text> : <Table className="mt-4">
        <TableHead>
          <TableRow>
            <TableHeaderCell>Date</TableHeaderCell>
            <TableHeaderCell>Type</TableHeaderCell>
            <TableHeaderCell>Identifier</TableHeaderCell>
            <TableHeaderCell>Direction</TableHeaderCell>
            <TableHeaderCell>Price / Unit</TableHeaderCell>
            <TableHeaderCell>Units</TableHeaderCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {data?.sort((a: any, b: any) => {
            a = new Date(a.date);
            b = new Date(b.date);
            return b - a;

          }).map((event: any) => (
            <TableRow key={event.date + event.units}>
              <TableCell>
                {formatDate(new Date(event.date))}
              </TableCell>
              <TableCell>
                {event.event_type}
              </TableCell>
              <TableCell>
                {event.identifier}
              </TableCell>
              <TableCell>
                {event.direction}
              </TableCell>
              <TableCell>
                {formatCurrency(event.price_unit, event.currency)}
              </TableCell>
              <TableCell>
                {event.units}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>}
    </div>
  )
}

export default Timeline;
