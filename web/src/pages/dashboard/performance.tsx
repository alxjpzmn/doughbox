import { useState } from "react";
import useSwr from "swr";
import PerformanceChart, { type HistoryMeasure } from "@/components/charts/portfolio-performance-chart";
import PositionPerformanceChart from "@/components/charts/position-performance-chart";
import { PortfolioOverview, PortfolioPerformance, PositionPerformance } from "@/types/core";
import EmptyState, { EmptyStateVariants } from "@/components/composite/empty-state";
import { CalculationRefresh } from "@/components/composite/calculation-refresh";
import { Skeleton } from "@/components/ui/skeleton";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Label } from "@/components/ui/label";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency, formatUnixTimestampRelative, formatRelativeAmount, colorMetric } from "@/lib/utils";

type SortMethod =
  | "ascTotalReturn"
  | "descTotalReturn"
  | "ascRealized"
  | "descRealized"
  | "ascUnrealized"
  | "descUnrealized"
  | "ascAlpha"
  | "descAlpha";

const sorting = (method: SortMethod): ((a: PositionPerformance, b: PositionPerformance) => number) => {
  const field = method.endsWith("TotalReturn") ? "total_return"
    : method.endsWith("Realized") ? "realized"
      : method.endsWith("Unrealized") ? "unrealized" : "alpha";
  const direction = method.startsWith("asc") ? 1 : -1;
  return (a, b) => {
    if (a[field] == null) return b[field] == null ? 0 : 1;
    if (b[field] == null) return -1;
    return direction * (Number(a[field]) - Number(b[field]));
  };
};

const Performance = () => {
  const { data, isLoading, error } = useSwr<PortfolioPerformance>(`${BASE_URL}/performance_overview`, fetcher);
  const [sortBy, setSortBy] = useState<SortMethod>("ascTotalReturn");
  const [showOnlyActivePositions, setShowOnlyActivePositions] = useState(false);
  const [historyMeasure, setHistoryMeasure] = useState<HistoryMeasure>('amount');
  const { data: activePortfolio, error: activePositionsError, isLoading: activePositionsLoading } = useSwr<PortfolioOverview>(
    showOnlyActivePositions ? `${BASE_URL}/portfolio` : null,
    fetcher,
  );
  const activeAssetIds = new Set(
    activePortfolio?.positions.filter((holding) => Number(holding.units) > 0).map((holding) => holding.asset_id),
  );
  const activeSelectionUnknown = showOnlyActivePositions && (activePositionsLoading || !activePortfolio);
  const activeHistoryIncomplete = showOnlyActivePositions && !!activePortfolio?.security_history_gaps.length;
  // filter creates a fresh array, so sorting never mutates SWR's cached response.
  const positions = (data?.position ?? [])
    .filter((position) => !showOnlyActivePositions || activeAssetIds.has(position.isin))
    .sort(sorting(sortBy));
  const visibleAlpha = data?.alpha == null ? null
    : !showOnlyActivePositions ? Number(data.alpha)
      : positions.every((position) => position.alpha != null && Number.isFinite(Number(position.alpha)))
        ? positions.reduce((total, position) => total + Number(position.alpha), 0)
        : null;

  return (
    <>
      <CalculationRefresh source="performance" />
      <div className="mb-6 flex flex-wrap items-center gap-3">
        <Switch id="performance-active-only" checked={showOnlyActivePositions} onCheckedChange={setShowOnlyActivePositions} />
        <Label htmlFor="performance-active-only">Active only</Label>
        <p className="text-sm text-muted-foreground">Applies to history, conviction, individual performance and visible alpha.</p>
      </div>
      {activeHistoryIncomplete ? (
        <p role="alert" className="rounded-xl border border-destructive/20 bg-destructive/10 p-4 text-sm text-destructive">
          Active holdings cannot be determined reliably because acquisition history is incomplete. All performance widgets are hidden. Reconcile the history gaps shown on Portfolio, or turn off Active only to view saved full-portfolio results.
        </p>
      ) : showOnlyActivePositions && activePositionsError ? (
        <p role="alert" className="rounded-xl border border-destructive/20 bg-destructive/10 p-4 text-sm text-destructive">
          Active holdings could not be loaded. All performance widgets are hidden until the active selection is available. Turn off Active only to view the full portfolio.
        </p>
      ) : activeSelectionUnknown ? (
        <p role="status" className="py-8 text-sm text-muted-foreground">Loading active holdings for all performance widgets…</p>
      ) : showOnlyActivePositions && activeAssetIds.size === 0 ? (
        <p className="py-8 text-sm text-muted-foreground">No active holdings. There is no active-only history, conviction, individual performance or alpha to display.</p>
      ) : error ? (
        error.details?.events_present === false ? (
          <EmptyState variant={EmptyStateVariants.WithCliInstructionImportTrades} docker={error.details?.in_docker} />
        ) : (
          <p role="alert" className="py-8 text-sm text-muted-foreground">
            Saved performance is unavailable. Press Refresh performance above to rebuild the exports and save a fresh history snapshot. Opening this page does not calculate performance.
          </p>
        )
      ) : isLoading || !data ? (
        <>
          {["History", "Conviction", "Individual performance"].map((title) => (
            <Card className="mb-6" key={title}>
              <CardHeader><Skeleton className="h-6 w-1/4" /><Skeleton className="h-4 w-1/2" /></CardHeader>
              <CardContent><Skeleton className="h-64 w-full" /></CardContent>
            </Card>
          ))}
        </>
      ) : (
        <>
          <Card className="grid grid-cols-1 gap-2">
            <CardHeader>
              <CardTitle>{showOnlyActivePositions ? "Active holdings" : "Portfolio"} P&amp;L history</CardTitle>
              <CardDescription>
                Last updated {formatUnixTimestampRelative(data.generated_at)}. P&amp;L includes trading gains after fees, recorded gross EUR security dividends and recorded cash/debt interest, before withholding tax; no projected income or deemed distributions. Percentages divide EUR P&amp;L by cumulative gross EUR purchases or contributions, including purchase fees; they are not annualized.
                {showOnlyActivePositions ? " History sums the recorded components of today's active holdings, not historical active holdings. Unlinked interest is excluded." : " Unlinked interest is included once for the full portfolio."}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <PerformanceChart activeOnly={showOnlyActivePositions} activeAssetIds={activeAssetIds}
                measure={historyMeasure} onMeasureChange={setHistoryMeasure} />
            </CardContent>
          </Card>

          <Card className="mt-6">
            <CardHeader>
              <CardTitle>Conviction vs. Result</CardTitle>
              <CardDescription>
                Securities P&amp;L includes trading gains after fees and recorded gross EUR dividends, before withholding tax.
                {visibleAlpha == null ? " Benchmark and alpha unavailable: benchmark data is missing." : ` ${showOnlyActivePositions ? "Visible" : "Total"} alpha: ${formatCurrency(visibleAlpha)}.`}
                {" "}The benchmark is a nominal price index with no reinvested benchmark dividends and no simulated fees, tax or FX.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <PositionPerformanceChart positions={positions} />
            </CardContent>
          </Card>

          <Card className="mt-6">
            <CardHeader>
              <CardTitle>Individual performance including recorded income</CardTitle>
              <CardDescription>Return includes realized trading P&amp;L, unrealized P&amp;L and recorded gross dividends. Realized remains trading-only; income is shown separately before withholding tax.</CardDescription>
            </CardHeader>
            <CardContent>
              <Select value={sortBy} onValueChange={(value) => setSortBy(value as SortMethod)}>
                <SelectTrigger className="w-full sm:w-72"><SelectValue placeholder="Sort by..." /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="ascTotalReturn">Total Return Ascending</SelectItem>
                  <SelectItem value="descTotalReturn">Total Return Descending</SelectItem>
                  <SelectItem value="ascRealized">Realized Ascending</SelectItem>
                  <SelectItem value="descRealized">Realized Descending</SelectItem>
                  <SelectItem value="ascUnrealized">Unrealized Ascending</SelectItem>
                  <SelectItem value="descUnrealized">Unrealized Descending</SelectItem>
                  <SelectItem value="ascAlpha">Alpha Ascending</SelectItem>
                  <SelectItem value="descAlpha">Alpha Descending</SelectItem>
                </SelectContent>
              </Select>
              {positions.length === 0 ? (
                <p className="py-8 text-sm text-muted-foreground">No securities performance for {showOnlyActivePositions ? "the active holdings" : "this portfolio"}. Manual holdings appear in P&amp;L history only.</p>
              ) : (
                <Table className="mt-4">
                  <TableHeader>
                    <TableRow>
                      <TableHead>Name</TableHead>
                      <TableHead>Return incl. income</TableHead>
                      <TableHead>Realized trading</TableHead>
                      <TableHead>Unrealized</TableHead>
                      <TableHead>Income</TableHead>
                      <TableHead>Alpha</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {positions.map((item) => (
                      <TableRow key={item.isin}>
                        <TableCell className="truncate overflow-hidden whitespace-nowrap max-w-48">
                          <a href={`https://duckduckgo.com/?q=${item.isin}`} target="_blank" rel="noreferrer">{item.name}</a>
                        </TableCell>
                        <TableCell><p className={Number(item.invested_amount) > 0 ? colorMetric(item.total_return) : "text-muted-foreground"} title={Number(item.invested_amount) > 0 ? undefined : "No recorded positive gross purchase baseline"}>{Number(item.invested_amount) > 0 ? formatRelativeAmount(Number(item.total_return)) : "Unavailable"}</p></TableCell>
                        <TableCell><p className={colorMetric(item.realized)}>{formatCurrency(Number(item.realized))}</p></TableCell>
                        <TableCell><p className={colorMetric(item.unrealized)}>{formatCurrency(Number(item.unrealized))}</p></TableCell>
                        <TableCell><p className={colorMetric(item.income)}>{formatCurrency(Number(item.income))}</p></TableCell>
                        <TableCell><p className={item.alpha == null ? "text-muted-foreground" : colorMetric(item.alpha)}>{item.alpha == null ? "Unavailable" : formatCurrency(Number(item.alpha))}</p></TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
          <Disclaimer />
        </>
      )}
    </>
  );
};

export default Performance;
