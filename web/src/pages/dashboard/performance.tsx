import { useEffect, useState } from "react";
import useSwr from "swr";
import PerformanceChart from "@/components/charts/portfolio-performance-chart";
import PositionPerformanceChart from "@/components/charts/position-performance-chart";
import { PortfolioPerformance, PositionPerformance } from "@/types/core";
import EmptyState, { EmptyStateVariants } from "@/components/composite/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Disclaimer } from "@/components/composite/disclaimer";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Label } from "@/components/ui/label";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency, formatUnixTimestampRelative, formatRelativeAmount } from "@/lib/utils";

interface PositionPerformanceWithKey extends PositionPerformance {
  key: string;
}

type sortByMethods =
  | "ascTotalReturn"
  | "descTotalReturn"
  | "ascRealized"
  | "descRealized"
  | "ascUnrealized"
  | "descUnrealized"
  | "ascAlpha"
  | "descAlpha";

const Performance = ({ }) => {
  const { data, isLoading, error } = useSwr<PortfolioPerformance>(`${BASE_URL}/performance_overview`, fetcher);

  const sorting = (method: sortByMethods): (a: PositionPerformance, b: PositionPerformance) => number => {
    switch (method) {
      case "ascTotalReturn":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(a.total_return) - parseFloat(b.total_return);
      case "descTotalReturn":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(b.total_return) - parseFloat(a.total_return);
      case "ascRealized":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(a.realized) - parseFloat(b.realized);
      case "descRealized":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(b.realized) - parseFloat(a.realized);
      case "ascUnrealized":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(a.unrealized) - parseFloat(b.unrealized);
      case "descUnrealized":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(b.unrealized) - parseFloat(a.unrealized);
      case "ascAlpha":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(a.alpha) - parseFloat(b.alpha);
      case "descAlpha":
        return (a: PositionPerformance, b: PositionPerformance) => parseFloat(b.alpha) - parseFloat(a.alpha);
      default:
        return () => 0;
    }
  };

  const [sortBy, setSortBy] = useState<sortByMethods>("ascTotalReturn");
  const [showOnlyActivePositions, setShowOnlyActivePositions] = useState(false);
  const [positions, setPositions] = useState<PositionPerformanceWithKey[]>([]);

  useEffect(() => {
    if (!isLoading && data) {
      setPositions(
        data?.position
          ?.filter((position) => {
            if (showOnlyActivePositions) {
              return position.unrealized !== "0.0";
            } else {
              return true;
            }
          })
          .sort(sorting(sortBy))
          .map((position) => {
            return {
              key: `${position.isin}-${position.total_return}`,
              unrealized: position.unrealized,
              realized: position.realized,
              total_return: position.total_return,
              alpha: position.alpha,
              name: position.name,
              isin: position.isin,
              performance: position.performance,
              invested_amount: position.invested_amount,
              simulated: position.simulated,
            };
          })
      );
    }
  }, [data, sortBy, isLoading, showOnlyActivePositions]);

  return (
    <>
      {error && !error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionImportTrades} docker={error.details?.in_docker} />}
      {error && error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionPerformance} docker={error.details?.in_docker} />}
      {isLoading ? (
        <>
          {/* Skeleton for Performance Card */}
          <Card className="grid grid-cols-1 gap-2">
            <CardHeader>
              <Skeleton className="h-6 w-1/4" />
              <Skeleton className="h-4 w-1/2" />
            </CardHeader>
            <CardContent>
              <Skeleton className="h-64 w-full" /> {/* Placeholder for PerformanceChart */}
            </CardContent>
          </Card>

          {/* Skeleton for Conviction vs. Result Card */}
          <Card className="mt-6">
            <CardHeader>
              <Skeleton className="h-6 w-1/4" /> {/* Placeholder for "Conviction vs. Result" text */}
            </CardHeader>
            <CardContent>
              <Skeleton className="h-64 w-full" /> {/* Placeholder for PositionPerformanceScatterChart */}
            </CardContent>
          </Card>

          {/* Skeleton for Individual Performance Card */}
          <Card className="mt-6">
            <CardHeader>
              <Skeleton className="h-6 w-1/4" /> {/* Placeholder for "Individual Performance" text */}
            </CardHeader>
            <CardContent>
              <div className="flex flex-col md:flex flex-wrap justify-between items-baseline gap-4">
                <div className="flex flex-col items-start gap-4">
                  <Skeleton className="h-10 w-full" /> {/* Placeholder for Select */}
                  <div className="flex items-center justify-center gap-2 min-w-max">
                    <Skeleton className="h-6 w-6" /> {/* Placeholder for Switch */}
                    <Skeleton className="h-6 w-20" /> {/* Placeholder for Label */}
                  </div>
                </div>
              </div>
              <Table className="mt-4">
                <TableHeader>
                  <TableRow>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                    <TableHead><Skeleton className="h-6 w-24" /></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: 10 }).map((_, index) => (
                    <TableRow key={index}>
                      <TableCell><Skeleton className="h-6 w-48" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                      <TableCell><Skeleton className="h-6 w-24" /></TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </>
      ) : data && (
        <>
          <Card className="grid grid-cols-1 gap-2">
            <CardHeader>
              <CardTitle>
                Performance
              </CardTitle>
              <CardDescription>
                Total alpha: {formatCurrency(parseFloat(data?.alpha))}, last updated {formatUnixTimestampRelative(data?.generated_at)}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <PerformanceChart />
            </CardContent>
          </Card>

          <Card className="mt-6">
            <CardHeader>
              <CardTitle>
                Conviction vs. Result
              </CardTitle>
            </CardHeader>
            <CardContent>
              <PositionPerformanceChart />
            </CardContent>
          </Card>

          <Card className="mt-6">
            <CardHeader>
              <CardTitle>
                Individual Performance
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-col md:flex flex-wrap justify-between items-baseline gap-4">
                <div className="flex flex-col items-start gap-4">
                  <Select
                    value={undefined}
                    defaultValue={sortBy}
                    //@ts-ignore
                    onValueChange={setSortBy}
                    placeholder="Sort by..."
                    className="w-full"
                  >
                    <SelectTrigger className="w-xs">
                      <SelectValue placeholder="Sort by..." />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="ascTotalReturn">Return Ascending</SelectItem>
                      <SelectItem value="descTotalReturn">Return Descending</SelectItem>
                      <SelectItem value="ascRealized">Realized Ascending</SelectItem>
                      <SelectItem value="descRealized">Realized Descending</SelectItem>
                      <SelectItem value="ascUnrealized">Unrealized Ascending</SelectItem>
                      <SelectItem value="descUnrealized">Unrealized Descending</SelectItem>
                      <SelectItem value="ascAlpha">Alpha Ascending</SelectItem>
                      <SelectItem value="descAlpha">Alpha Descending</SelectItem>
                    </SelectContent>
                  </Select>
                  <div className="flex items-center justify-center gap-2 min-w-max">
                    <Switch id="r1" checked={showOnlyActivePositions} onCheckedChange={() => setShowOnlyActivePositions(!showOnlyActivePositions)} />
                    <Label htmlFor="r1">Active only</Label>
                  </div>
                </div>
              </div>
              <Table className="mt-4">
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Total Return</TableHead>
                    <TableHead>Realized</TableHead>
                    <TableHead>Unrealized</TableHead>
                    <TableHead>Alpha</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {positions?.map((item) => (
                    <TableRow key={item.key}>
                      <TableCell className="truncate overflow-hidden whitespace-nowrap max-w-48">
                        <a
                          href={`https://duckduckgo.com/?q=${item.isin}`}
                          target="_blank"
                        >
                          {item.name}
                        </a>
                      </TableCell>
                      <TableCell>
                        <p
                          className={
                            parseFloat(item.total_return) === 0
                              ? "text-muted-foreground"
                              : parseFloat(item.total_return) < 0
                                ? "text-destructive-foreground"
                                : "text-success-foreground"
                          }
                        >
                          {formatRelativeAmount(parseFloat(item.total_return))}
                        </p>
                      </TableCell>
                      <TableCell>
                        <p
                          color={
                            parseFloat(item.realized) === 0
                              ? "gray"
                              : parseFloat(item.realized) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.realized))}
                        </p>
                      </TableCell>
                      <TableCell>
                        <p
                          color={
                            parseFloat(item.unrealized) === 0
                              ? "gray"
                              : parseFloat(item.unrealized) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.unrealized))}
                        </p>
                      </TableCell>
                      <TableCell>
                        <p
                          color={
                            parseFloat(item.alpha) === 0
                              ? "gray"
                              : parseFloat(item.alpha) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.alpha))}
                        </p>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>

          <Disclaimer />
        </>
      )}
    </>
  );
};

export default Performance;
