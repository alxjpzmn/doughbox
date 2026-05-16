import useSwr from "swr";
import { useState } from "react";
import { format } from "date-fns";
import { AnnualTaxableAmounts, SecWac, TaxationReport, FxWac } from "@/types/core";
import EmptyState, { EmptyStateVariants } from "@/components/composite/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from "@/components/ui/table";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Calendar } from "@/components/ui/calendar";
import { CalendarIcon, X, Download } from "lucide-react";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency } from "@/lib/utils";

const labelMap: Record<keyof AnnualTaxableAmounts, string> = {
  cash_interest: "Cash Interest",
  share_lending_interest: "Share Lending Interest",
  capital_gains: "Capital Gains",
  capital_losses: "Capital Losses",
  dividends: "Dividends",
  dividend_equivalents: "Dividend Equivalents",
  fx_appreciation: "FX Appreciation",
  withheld_tax_capital_gains: "Withheld Tax (Capital Gains)",
  withheld_tax_dividends: "Withheld Tax (Dividends)",
  withheld_tax_interest: "Withheld Tax (Interest)",
  tax_optimization_adjustment: "Tax Optimization",
};

const Taxation = () => {
  const [fromDate, setFromDate] = useState<Date | undefined>(undefined);
  const [untilDate, setUntilDate] = useState<Date | undefined>(undefined);

  const queryParams = new URLSearchParams();
  if (fromDate) queryParams.set("from_date", format(fromDate, "yyyy-LL-dd"));
  if (untilDate) queryParams.set("until_date", format(untilDate, "yyyy-LL-dd"));
  const queryString = queryParams.toString();
  const url = queryString ? `${BASE_URL}/taxation?${queryString}` : `${BASE_URL}/taxation`;

  const { data, error, isLoading } = useSwr<TaxationReport>(url, fetcher);

  const isFiltered = fromDate !== undefined || untilDate !== undefined;

  const downloadDetailed = async () => {
    const qp = new URLSearchParams();
    if (fromDate) qp.set("from_date", format(fromDate, "yyyy-LL-dd"));
    if (untilDate) qp.set("until_date", format(untilDate, "yyyy-LL-dd"));
    const qs = qp.toString();
    const response = await fetch(`${BASE_URL}/taxation/detailed${qs ? `?${qs}` : ""}`);
    if (!response.ok) {
      console.error("Failed to download detailed report", response.statusText);
      return;
    }
    const blob = await response.blob();
    const downloadUrl = window.URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = downloadUrl;
    a.download = `taxation_detailed${fromDate ? `_from_${format(fromDate, "yyyy-LL-dd")}` : ""}${untilDate ? `_until_${format(untilDate, "yyyy-LL-dd")}` : ""}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    window.URL.revokeObjectURL(downloadUrl);
  };

  return (
    <div>
      <Card className="w-full flex flex-col justify-start mb-6">
        <CardHeader>
          <CardTitle>Date Range</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex flex-wrap items-center gap-4">
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">From</span>
              <Popover>
                <PopoverTrigger asChild>
                  <Button
                    variant={"outline"}
                    className={cn(
                      "w-52 justify-start text-left font-normal",
                      !fromDate && "text-muted-foreground",
                    )}
                  >
                    <CalendarIcon />
                    {fromDate ? format(fromDate, "PPP") : <span>Pick a date</span>}
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-auto p-0" align="start">
                  <Calendar
                    mode="single"
                    selected={fromDate}
                    // @ts-ignore
                    onSelect={setFromDate}
                  />
                </PopoverContent>
              </Popover>
              {fromDate && (
                <Button variant="ghost" size="icon" onClick={() => setFromDate(undefined)}>
                  <X className="h-4 w-4" />
                </Button>
              )}
            </div>

            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Until</span>
              <Popover>
                <PopoverTrigger asChild>
                  <Button
                    variant={"outline"}
                    className={cn(
                      "w-52 justify-start text-left font-normal",
                      !untilDate && "text-muted-foreground",
                    )}
                  >
                    <CalendarIcon />
                    {untilDate ? format(untilDate, "PPP") : <span>Pick a date</span>}
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-auto p-0" align="start">
                  <Calendar
                    mode="single"
                    selected={untilDate}
                    // @ts-ignore
                    onSelect={setUntilDate}
                  />
                </PopoverContent>
              </Popover>
              {untilDate && (
                <Button variant="ghost" size="icon" onClick={() => setUntilDate(undefined)}>
                  <X className="h-4 w-4" />
                </Button>
              )}
            </div>

            {isFiltered && (
              <Button variant="outline" size="sm" onClick={() => { setFromDate(undefined); setUntilDate(undefined); }}>
                Clear
              </Button>
            )}
            <Button variant="outline" size="sm" onClick={downloadDetailed}>
              <Download className="h-4 w-4 mr-1" />
              Export Detailed
            </Button>
          </div>
        </CardContent>
      </Card>

      {error && !error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionImport} docker={error.details?.in_docker} />}
      {error && error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionTaxation} docker={error.details?.in_docker} />}
      {isLoading ? (
        <div className="grid grid-cols-1 gap-4">
          <Skeleton className="h-6 w-1/4 mb-4" />

          {Array.from({ length: 3 }).map((_, index) => (
            <Card key={index}>
              <CardHeader>
                <Skeleton className="h-6 w-1/4" />
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                      <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {Array.from({ length: 5 }).map((_, idx) => (
                      <TableRow key={idx}>
                        <TableCell><Skeleton className="h-6 w-48" /></TableCell>
                        <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          ))}

          <Card>
            <CardHeader>
              <Skeleton className="h-6 w-1/4" />
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: 3 }).map((_, idx) => (
                    <TableRow key={idx}>
                      <TableCell><Skeleton className="h-6 w-48" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </div>
      ) : (
        <div>
          {isFiltered && (
            <p className="text-sm text-muted-foreground mb-4">
              Showing realized gains
              {fromDate && <> from {format(fromDate, "PPP")}</>}
              {untilDate && <> until {format(untilDate, "PPP")}</>}
            </p>
          )}

          {data?.taxable_amounts && Object.entries(data.taxable_amounts).map(([year, amounts]) => (
            <Card key={year} className="mb-4">
              <CardHeader>
                <CardTitle>{year}</CardTitle>
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Item</TableHead>
                      <TableHead>Amount</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {Object.entries(amounts as AnnualTaxableAmounts).map(([key, value]) => (
                      <TableRow key={key}>
                        <TableCell>{labelMap[key as keyof AnnualTaxableAmounts]}</TableCell>
                        <TableCell>{formatCurrency(value)}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          ))}

          <Card className="mb-4">
            <CardHeader>
              <CardTitle>Instrument WAC</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Units</TableHead>
                    <TableHead>WAC</TableHead>
                    <TableHead>WAC FX</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(data?.securities_wacs as SecWac[])?.map(([key, value]) => (
                    <TableRow key={key}>
                      <TableCell className="truncate overflow-hidden whitespace-nowrap max-w-48">
                        {value.name}
                      </TableCell>
                      <TableCell>{value.units}</TableCell>
                      <TableCell>{formatCurrency(parseFloat(value.average_cost))}</TableCell>
                      <TableCell>{formatCurrency(parseFloat(value.weighted_avg_fx_rate))}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Currency WAC</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Units</TableHead>
                    <TableHead>WAC</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(data?.currency_wacs as FxWac[])?.map(([key, value]) => (
                    <TableRow key={key}>
                      <TableCell>{key}</TableCell>
                      <TableCell>{value.units}</TableCell>
                      <TableCell>{value.avg_rate}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
          <Disclaimer />
        </div>
      )}
    </div>
  );
};

export default Taxation;
