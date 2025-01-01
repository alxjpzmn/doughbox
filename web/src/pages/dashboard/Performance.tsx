import { useEffect, useState } from "react";
import {
  BASE_URL,
  formatCurrency,
  fetcher,
  formatUnixTimestampRelative,
  formatRelativeAmount,
} from "@/util";
import useSwr from "swr";
import PerformanceChart from "@/components/Charts/PortfolioPerformanceChart";
import {
  Card,
  Flex,
  Select,
  SelectItem,
  Text,
} from "@tremor/react";
import PositionPerformanceScatterChart from "@/components/Charts/PositionPerformanceChart";
import { Switch } from "@/components/Switch";
import { Label } from "@/components/Label";
import { PortfolioPerformance, PositionPerformance } from "@/types/core";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "@/components/Table";

interface PositionPerformanceWithKey extends PositionPerformance {
  key: string
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
  const { data, isLoading } = useSwr<PortfolioPerformance>(`${BASE_URL}/performance_overview`, fetcher);

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
        return () => { return 0 };
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
      {data &&
        <>
          <Card className="grid grid-cols-1 gap-2">
            <Flex className="justify-between items-baseline truncate">
              <Text>Performance</Text>
              <Text>Total alpha: {formatCurrency(parseFloat(data?.alpha))}</Text>
            </Flex>
            <PerformanceChart />
            <Flex className="justify-between items-baseline truncate">
              <Text color="gray">
                Last updated {formatUnixTimestampRelative(data?.generated_at)}
              </Text>
            </Flex>
          </Card>

          <Card className="mt-6">
            <Text>Conviction vs. Result</Text>
            <PositionPerformanceScatterChart />
          </Card>

          <Card className="mt-6">
            <Flex className="flex-col md:flex flex-wrap justify-between items-baseline gap-4">
              <Text className="flex-grow">Individiual Performance</Text>
              <Flex className="flex md:justify-between gap-16">
                <Select
                  value={undefined}
                  defaultValue={sortBy}
                  //@ts-ignore
                  onValueChange={setSortBy}
                  placeholder="Sort by..."
                  className="max-w-full"
                >
                  <SelectItem value="ascTotalReturn">Return Ascending</SelectItem>
                  <SelectItem value="descTotalReturn">Return Descending</SelectItem>
                  <SelectItem value="ascRealized">
                    Realized Ascending
                  </SelectItem>
                  <SelectItem value="descRealized">
                    Realized Descending
                  </SelectItem>
                  <SelectItem value="ascUnrealized">
                    Unrealized Ascending
                  </SelectItem>
                  <SelectItem value="descUnrealized">
                    Unrealized Descending
                  </SelectItem>
                  <SelectItem value="ascAlpha">Alpha Ascending</SelectItem>
                  <SelectItem value="descAlpha">Alpha Descending</SelectItem>
                </Select>
                <div className="flex items-center justify-center gap-2 min-w-max">
                  <Switch id="r1" checked={showOnlyActivePositions} onCheckedChange={() => setShowOnlyActivePositions(!showOnlyActivePositions)} />
                  <Label htmlFor="r1">Active only</Label>
                </div>
              </Flex>
            </Flex>
            {!isLoading && (
              <Table className="mt-4">
                <TableHead>
                  <TableRow>
                    <TableHeaderCell>Name</TableHeaderCell>
                    <TableHeaderCell>Total Return</TableHeaderCell>
                    <TableHeaderCell>Realized</TableHeaderCell>
                    <TableHeaderCell>Unrealized</TableHeaderCell>
                    <TableHeaderCell>Alpha</TableHeaderCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {positions?.map((item) => (
                    <TableRow key={item.key}>
                      <TableCell className="max-w-6 md:max-w-36 truncate">
                        <a
                          href={`https://duckduckgo.com/?q=${item.isin}`}
                          target="_blank"
                        >
                          {item.name}
                        </a>
                      </TableCell>
                      <TableCell>
                        <Text
                          color={
                            parseFloat(item.total_return) === 0
                              ? "gray"
                              : parseFloat(item.total_return) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatRelativeAmount(parseFloat(item.total_return))}
                        </Text>
                      </TableCell>
                      <TableCell>
                        <Text
                          color={
                            parseFloat(item.realized) === 0
                              ? "gray"
                              : parseFloat(item.realized) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.realized))}
                        </Text>
                      </TableCell>
                      <TableCell>
                        <Text
                          color={
                            parseFloat(item.unrealized) === 0
                              ? "gray"
                              : parseFloat(item.unrealized) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.unrealized))}
                        </Text>
                      </TableCell>
                      <TableCell>
                        <Text
                          color={
                            parseFloat(item.alpha) === 0
                              ? "gray"
                              : parseFloat(item.alpha) < 0
                                ? "red"
                                : "green"
                          }
                        >
                          {formatCurrency(parseFloat(item.alpha))}
                        </Text>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </Card></>}
    </>
  );
};

export default Performance;
