import useSwr from "swr";
import { useState, useMemo } from "react";
import { format } from "date-fns";
import { AnnualTaxableAmounts, SecWac, TaxationReport, FxWac, TransactionTaxImpact, EventType } from "@/types/core";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { Calendar } from "@/components/ui/calendar";
import { CalendarIcon, X, Download, ArrowDown, ArrowUp, ChevronsUpDown } from "lucide-react";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency, colorMetric } from "@/lib/utils";

const labelMap: Record<keyof AnnualTaxableAmounts, string> = {
  cash_interest: "Cash Interest",
  share_lending_interest: "Share Lending Interest",
  capital_gains: "Capital Gains",
  capital_losses: "Capital Losses",
  net_capital_gains: "Net Capital Gains (after loss offset)",
  dividends: "Dividends",
  dividend_equivalents: "Dividend Equivalents",
  fx_appreciation: "FX Appreciation",
  withheld_tax_capital_gains: "Withheld Tax (Capital Gains)",
  withheld_tax_dividends: "Withheld Tax (Dividends)",
  withheld_tax_interest: "Withheld Tax (Interest)",
  tax_optimization_adjustment: "Tax Optimization",
  tax_owed_dividends: "Tax Owed (Dividends)",
  tax_owed_dividend_equivalents: "Tax Owed (Dividend Equivalents)",
};

const formFields: { key: keyof AnnualTaxableAmounts; kz: string }[] = [
  { key: "cash_interest", kz: "KZ 465" },
  { key: "share_lending_interest", kz: "KZ 897/898" },
  { key: "capital_gains", kz: "KZ 731" },
  { key: "capital_losses", kz: "KZ 732" },
  { key: "dividends", kz: "KZ 897/898" },
  { key: "dividend_equivalents", kz: "KZ 936/937" },
  { key: "fx_appreciation", kz: "KZ 731" },
  { key: "withheld_tax_dividends", kz: "KZ 984/998" },
];

const infoFields: { key: keyof AnnualTaxableAmounts; kz: string }[] = [
  { key: "net_capital_gains", kz: "" },
  { key: "withheld_tax_capital_gains", kz: "" },
  { key: "withheld_tax_interest", kz: "" },
  { key: "tax_optimization_adjustment", kz: "" },
  { key: "tax_owed_dividends", kz: "" },
  { key: "tax_owed_dividend_equivalents", kz: "" },
];

type SortColumn =
  | "date"
  | "event_type"
  | "broker"
  | "identifier"
  | "direction"
  | "units"
  | "price_unit"
  | "total"
  | "impact_type"
  | "taxable_amount"
  | "withheld_tax"
  | "tax_liability"
  | "tax_rate_percent"
  | "source_country";

type SortDirection = "asc" | "desc";

const eventTypeLabels: Record<EventType, string> = {
  [EventType.CashInterest]: "Interest",
  [EventType.ShareInterest]: "Share Interest",
  [EventType.Dividend]: "Dividend",
  [EventType.Trade]: "Trade",
  [EventType.FxConversion]: "FX",
  [EventType.DividendAequivalent]: "Div. Equivalent",
};

const Taxation = () => {
  const [fromDate, setFromDate] = useState<Date | undefined>(undefined);
  const [untilDate, setUntilDate] = useState<Date | undefined>(undefined);
  const [activeTab, setActiveTab] = useState<"summary" | "transactions">("summary");

  const queryParams = new URLSearchParams();
  if (fromDate) queryParams.set("from_date", format(fromDate, "yyyy-LL-dd"));
  if (untilDate) queryParams.set("until_date", format(untilDate, "yyyy-LL-dd"));
  const queryString = queryParams.toString();

  const summaryUrl = queryString ? `${BASE_URL}/taxation?${queryString}` : `${BASE_URL}/taxation`;
  const transactionsUrl = queryString ? `${BASE_URL}/taxation/transactions?${queryString}` : `${BASE_URL}/taxation/transactions`;

  const { data, error, isLoading } = useSwr<TaxationReport>(summaryUrl, fetcher);
  const {
    data: txData,
    error: txError,
    isLoading: txIsLoading,
  } = useSwr<TransactionTaxImpact[]>(transactionsUrl, fetcher);

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

      <Tabs value={activeTab} onValueChange={(v) => setActiveTab(v as "summary" | "transactions")}>
        <TabsList className="mb-6">
          <TabsTrigger value="summary">Summary</TabsTrigger>
          <TabsTrigger value="transactions">Transactions</TabsTrigger>
        </TabsList>

        <TabsContent value="summary">
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
                          <TableHead>KZ</TableHead>
                          <TableHead className="text-right">Amount</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {formFields.map(({ key, kz }) => (
                          <TableRow key={key}>
                            <TableCell>{labelMap[key]}</TableCell>
                            <TableCell className="font-mono text-xs text-muted-foreground">{kz}</TableCell>
                            <TableCell className="text-right">{formatCurrency(parseFloat((amounts as AnnualTaxableAmounts)[key] as string))}</TableCell>
                          </TableRow>
                        ))}
                        <TableRow className="border-t-2">
                          <TableCell colSpan={3} className="text-xs uppercase tracking-wide text-muted-foreground font-medium pt-3 pb-1">Reconciliation</TableCell>
                        </TableRow>
                        {infoFields.map(({ key }) => (
                          <TableRow key={key} className="text-muted-foreground">
                            <TableCell className="text-sm">{labelMap[key]}</TableCell>
                            <TableCell></TableCell>
                            <TableCell className="text-right text-sm">{formatCurrency(parseFloat((amounts as AnnualTaxableAmounts)[key] as string))}</TableCell>
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
        </TabsContent>

        <TabsContent value="transactions">
          <TransactionTaxView
            data={txData}
            error={txError}
            isLoading={txIsLoading}
            isFiltered={isFiltered}
            fromDate={fromDate}
            untilDate={untilDate}
          />
        </TabsContent>
      </Tabs>
    </div>
  );
};

interface TransactionTaxViewProps {
  data?: TransactionTaxImpact[];
  error?: any;
  isLoading: boolean;
  isFiltered: boolean;
  fromDate?: Date;
  untilDate?: Date;
}

const SortableHead = ({
  column,
  label,
  sortBy,
  sortDir,
  onSort,
  align = "left",
}: {
  column: SortColumn;
  label: string;
  sortBy: SortColumn;
  sortDir: SortDirection;
  onSort: (c: SortColumn) => void;
  align?: "left" | "right";
}) => {
  const active = sortBy === column;
  return (
    <TableHead className={cn(align === "right" && "text-right")}>
      <button
        type="button"
        onClick={() => onSort(column)}
        className={cn(
          "inline-flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors",
          align === "right" && "flex-row-reverse",
          active && "text-foreground font-medium",
        )}
      >
        {label}
        {active ? (
          sortDir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />
        ) : (
          <ChevronsUpDown className="h-3 w-3 opacity-40" />
        )}
      </button>
    </TableHead>
  );
};

const TransactionTaxView = ({ data, error, isLoading, isFiltered, fromDate, untilDate }: TransactionTaxViewProps) => {
  const [sortBy, setSortBy] = useState<SortColumn>("date");
  const [sortDir, setSortDir] = useState<SortDirection>("desc");
  const [taxRelevantOnly, setTaxRelevantOnly] = useState(true);

  const handleSort = (column: SortColumn) => {
    if (column === sortBy) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortBy(column);
      setSortDir("desc");
    }
  };

  const filteredAndSorted = useMemo(() => {
    if (!data) return [];

    let items = [...data];
    if (taxRelevantOnly) {
      items = items.filter((item) => item.is_tax_relevant);
    }

    items.sort((a, b) => {
      const get = (item: TransactionTaxImpact): number | string => {
        switch (sortBy) {
          case "date": return new Date(item.date).getTime();
          case "event_type": return item.event_type;
          case "broker": return item.broker ?? "";
          case "identifier": return item.identifier ?? "";
          case "direction": return item.direction ?? "";
          case "units": return parseFloat(item.units);
          case "price_unit": return parseFloat(item.price_unit);
          case "total": return parseFloat(item.total);
          case "impact_type": return item.impact_type ?? "";
          case "taxable_amount": return parseFloat(item.taxable_amount);
          case "withheld_tax": return parseFloat(item.withheld_tax);
          case "tax_liability": return parseFloat(item.tax_liability);
          case "tax_rate_percent": return parseFloat(item.tax_rate_percent);
          case "source_country": return item.source_country ?? "";
        }
      };

      const av = get(a);
      const bv = get(b);
      const cmp =
        typeof av === "number" && typeof bv === "number"
          ? av - bv
          : String(av).localeCompare(String(bv));
      return sortDir === "asc" ? cmp : -cmp;
    });

    return items;
  }, [data, sortBy, sortDir, taxRelevantOnly]);

  if (error) {
    return (
      <EmptyState
        variant={EmptyStateVariants.WithCliInstructionTaxation}
        docker={error.details?.in_docker}
      />
    );
  }

  return (
    <div>
      <div className="flex flex-wrap items-center gap-4 mb-4">
        <div className="flex items-center gap-2">
          <Switch
            checked={taxRelevantOnly}
            onCheckedChange={setTaxRelevantOnly}
            id="tax-relevant"
          />
          <label htmlFor="tax-relevant" className="text-sm text-muted-foreground cursor-pointer">
            Tax relevant only
          </label>
        </div>
      </div>

      {isFiltered && (
        <p className="text-sm text-muted-foreground mb-4">
          Showing transactions
          {fromDate && <> from {format(fromDate, "PPP")}</>}
          {untilDate && <> until {format(untilDate, "PPP")}</>}
        </p>
      )}

      <p className="text-sm text-muted-foreground mb-4">
        {filteredAndSorted.length} {filteredAndSorted.length === 1 ? "transaction" : "transactions"}
        {taxRelevantOnly ? " (tax relevant)" : ""}
        <span className="text-xs italic"> &mdash; not tax advice, always double-check.</span>
      </p>

      {isLoading ? (
        <Card>
          <CardContent className="pt-6">
            <div className="space-y-3">
              {Array.from({ length: 5 }).map((_, i) => (
                <div key={i} className="flex gap-4">
                  <Skeleton className="h-6 w-24" />
                  <Skeleton className="h-6 w-32" />
                  <Skeleton className="h-6 w-20" />
                  <Skeleton className="h-6 w-24" />
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      ) : (
        <div className="overflow-x-auto">
          <Table className="[&_td]:whitespace-nowrap">
            <TableHeader>
              <TableRow>
                <SortableHead column="date" label="Date" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="event_type" label="Type" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="broker" label="Broker" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="identifier" label="Identifier" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="direction" label="Dir." sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="units" label="Units" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="price_unit" label="Price" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="total" label="Total" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="impact_type" label="Impact" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="source_country" label="Src" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="taxable_amount" label="Taxable" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="withheld_tax" label="WHT" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="tax_liability" label="Tax left" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <SortableHead column="tax_rate_percent" label="Rate" align="right" sortBy={sortBy} sortDir={sortDir} onSort={handleSort} />
                <TableHead>Notes</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredAndSorted.map((item, idx) => {
                const taxable = parseFloat(item.taxable_amount);
                const withheldTax = parseFloat(item.withheld_tax);
                const taxLiability = parseFloat(item.tax_liability);
                const isGain = taxable > 0;
                const isLoss = taxable < 0;

                return (
                  <TableRow key={idx}>
                    <TableCell>
                      {format(new Date(item.date), "yyyy-LL-dd")}
                    </TableCell>
                    <TableCell>
                      <span className="inline-flex items-center gap-1">
                        {item.direction === "Buy" && <ArrowUp className="h-3 w-3 text-muted-foreground" />}
                        {item.direction === "Sell" && <ArrowDown className="h-3 w-3 text-muted-foreground" />}
                        {eventTypeLabels[item.event_type]}
                      </span>
                    </TableCell>
                    <TableCell>{item.broker}</TableCell>
                    <TableCell className="max-w-32 truncate" title={item.identifier || "-"}>
                      {item.identifier || "-"}
                    </TableCell>
                    <TableCell>{item.direction || "-"}</TableCell>
                    <TableCell className="text-right">{parseFloat(item.units).toFixed(2)}</TableCell>
                    <TableCell className="text-right">{formatCurrency(parseFloat(item.price_unit))} {item.currency}</TableCell>
                    <TableCell className="text-right">{formatCurrency(parseFloat(item.total))}</TableCell>
                    <TableCell>
                      <span
                        className={cn(
                          "inline-block px-2 py-0.5 rounded text-xs font-medium",
                          isGain && "bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-100",
                          isLoss && "bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-100",
                          !isGain && !isLoss && "bg-muted text-muted-foreground",
                        )}
                      >
                        {item.impact_type}
                      </span>
                    </TableCell>
                    <TableCell>
                      {item.source_country ? (
                        <span
                          className="inline-block px-1.5 py-0.5 rounded text-xs font-mono font-medium bg-muted"
                          title={item.dtt_rate_percent != null ? `DTT: ${item.dtt_rate_percent}%` : "No DTT (fallback)"}
                        >
                          {item.source_country}
                        </span>
                      ) : (
                        <span className="text-muted-foreground text-xs">—</span>
                      )}
                    </TableCell>
                    <TableCell className={cn("text-right font-medium", colorMetric(item.taxable_amount))}>
                      {taxable !== 0 ? formatCurrency(taxable) : "-"}
                    </TableCell>
                    <TableCell className="text-right text-muted-foreground">
                      {withheldTax > 0 ? formatCurrency(withheldTax) : "-"}
                    </TableCell>
                    <TableCell className="text-right">
                      {taxLiability > 0 ? formatCurrency(taxLiability) : "-"}
                    </TableCell>
                    <TableCell className="text-right text-muted-foreground">
                      {parseFloat(item.tax_rate_percent) > 0 ? `${item.tax_rate_percent}%` : "-"}
                    </TableCell>
                    <TableCell className="max-w-xs truncate" title={item.notes}>
                      <span className="text-sm text-muted-foreground">{item.notes}</span>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
          {!filteredAndSorted.length && !isLoading && (
            <div className="text-center py-12 text-muted-foreground">
              No transactions match the current filters.
            </div>
          )}
        </div>
      )}
      <Disclaimer />
    </div>
  );
};

export default Taxation;
