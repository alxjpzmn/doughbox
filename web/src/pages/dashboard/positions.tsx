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

const ALL_BROKERS = "all";

const Positions = () => {
  const [selectedDate, setSelectedDate] = useState<Date>(new Date());
  const [broker, setBroker] = useState(ALL_BROKERS);
  const { data: brokers } = useSwr<string[]>(`${BASE_URL}/brokers`, fetcher);
  const date = format(selectedDate, "yyyy-LL-dd");
  const { data, isLoading, error } = useSwr<PositionWithName[]>(
    ["positions", date, broker],
    async () => {
      const params = new URLSearchParams({ date });
      if (broker !== ALL_BROKERS) params.set("broker", broker);
      const res = await fetch(`${BASE_URL}/positions?${params.toString()}`, { cache: "no-store" });
      if (!res.ok) throw new Error("Positions could not be loaded.");
      return res.json();
    },
  );

  return (
    <div>
      <Card className="w-full flex flex-col justify-start mb-6">
        <CardHeader>
          <CardTitle>Filters</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4 sm:flex-row">
          <div className="space-y-2">
            <p className="text-sm font-medium">Date</p>
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
          </div>
          <div className="space-y-2">
            <p className="text-sm font-medium">Broker</p>
            <select
              value={broker}
              onChange={(event) => setBroker(event.target.value)}
              className="border-input bg-background h-9 w-60 cursor-pointer rounded-md border px-3 text-sm shadow-xs"
            >
              <option value={ALL_BROKERS}>All brokers</option>
              {(brokers ?? []).map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </div>
        </CardContent>
      </Card>

      {error && (
        <div role="alert" className="bg-destructive/10 text-destructive mb-6 rounded-xl border border-destructive/20 p-4 text-sm">
          Positions could not be loaded.
        </div>
      )}

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
      ) : data?.length === 0 && broker === ALL_BROKERS ? (
        <EmptyState />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>
                {broker === ALL_BROKERS ? "Positions" : `Positions · ${broker}`}
              </CardTitle>
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
                  {data?.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={2} className="text-muted-foreground">
                        No open positions for this broker.
                      </TableCell>
                    </TableRow>
                  ) : data?.map((item) => (
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
