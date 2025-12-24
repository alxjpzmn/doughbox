import useSwr from "swr";
import { useState } from "react";
import { format } from "date-fns";
import { PositionWithName } from "@/types/core";
import EmptyState from "@/components/composite/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Popover, PopoverContent } from "@/components/ui/popover";
import { PopoverTrigger } from "@radix-ui/react-popover";
import { cn } from "@/lib/utils";
import { CalendarIcon } from "lucide-react";
import { Calendar } from "@/components/ui/calendar";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { BASE_URL, fetcher } from "@/lib/http";

const Positions = () => {
  const [selectedDate, setSelectedDate] = useState<Date>(new Date());
  const { data, isLoading } = useSwr<PositionWithName[]>(
    `${BASE_URL}/positions?date=${format(selectedDate, "yyyy-LL-dd")}`,
    fetcher,
  );

  return (
    <div>
      {/* Date Picker Card */}
      <Card className="w-full flex flex-col justify-start mb-6">
        <CardHeader>
          <CardTitle>Date</CardTitle>
        </CardHeader>
        <CardContent>
          <Popover>
            <PopoverTrigger asChild>
              <Button
                variant={"outline"}
                className={cn(
                  "w-60 justify-start text-left font-normal",
                  !selectedDate && "text-muted-foreground",
                )}
              >
                <CalendarIcon />
                {selectedDate ? (
                  format(selectedDate, "PPP")
                ) : (
                  <span>Pick a date</span>
                )}
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-auto p-0" align="start">
              <Calendar
                mode="single"
                selected={selectedDate}
                required
                // @ts-ignore
                onSelect={setSelectedDate}
              />
            </PopoverContent>
          </Popover>
        </CardContent>
      </Card>

      {/* Skeleton Loader or Data */}
      {isLoading ? (
        <Card>
          <CardHeader>
            <Skeleton className="h-6 w-1/4" />{" "}
            {/* Placeholder for Table Header */}
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>
                    <Skeleton className="h-4 w-24" />
                  </TableHead>{" "}
                  {/* Placeholder for "Identifier" */}
                  <TableHead>
                    <Skeleton className="h-4 w-24" />
                  </TableHead>{" "}
                  {/* Placeholder for "Units" */}
                </TableRow>
              </TableHeader>
              <TableBody>
                {Array.from({ length: 20 }).map((_, index) => (
                  <TableRow key={index}>
                    <TableCell>
                      <Skeleton className="h-6 w-48" />{" "}
                      {/* Placeholder for Position Name */}
                    </TableCell>
                    <TableCell>
                      <Skeleton className="h-6 w-24" />{" "}
                      {/* Placeholder for Units */}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      ) : data?.length === 0 ? (
        <EmptyState />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Positions</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Identifier</TableHead>
                    <TableHead>Units</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {data?.map((item) => (
                    <TableRow key={`${item?.isin}`}>
                      <TableCell>
                        <a
                          href={`https://duckduckgo.com/?q=${item?.isin}`}
                          className="truncate overflow-hidden whitespace-nowrap max-w-fit"
                        >
                          {item?.name}
                        </a>
                      </TableCell>
                      <TableCell>{item?.units}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
          <Disclaimer />
        </>
      )}
    </div>
  );
};

export default Positions;
