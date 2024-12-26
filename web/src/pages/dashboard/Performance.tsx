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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Text,
} from "@tremor/react";
import PositionPerformanceScatterChart from "@/components/Charts/PositionPerformanceChart";
import { Switch } from "@/components/Switch";
import { Label } from "@/components/Label";


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
  const { data, isLoading } = useSwr(`${BASE_URL}/performance_overview`, fetcher);

  const sorting = (method: sortByMethods) => {
    switch (method) {
      case "ascTotalReturn":
        return (a: any, b: any) => a.total_return - b.total_return;
      case "descTotalReturn":
        return (a: any, b: any) => b.total_return - a.total_return;
      case "ascRealized":
        return (a: any, b: any) => a.realized - b.realized;
      case "descRealized":
        return (a: any, b: any) => b.realized - a.realized;
      case "ascUnrealized":
        return (a: any, b: any) => a.unrealized - b.unrealized;
      case "descUnrealized":
        return (a: any, b: any) => b.unrealized - a.unrealized;
      case "ascAlpha":
        return (a: any, b: any) => a.alpha - b.alpha;
      case "descAlpha":
        return (a: any, b: any) => b.alpha - a.alpha;
      default:
        return null;
    }
  };

  const [sortBy, setSortBy] = useState<sortByMethods>("ascTotalReturn");
  const [showOnlyActivePositions, setShowOnlyActivePositions] = useState(false);
  const [positions, setPositions] = useState([]);

  useEffect(() => {
    if (!isLoading) {
      setPositions(
        data?.position
          .map((position: any) => {
            return {
              key: `${position.isin}-${position.total_return}`,
              unrealized: parseFloat(position.unrealized),
              realized: parseFloat(position.realized),
              total_return: parseFloat(position.total_return),
              alpha: parseFloat(position.alpha),
              name: position.name
            };
          })
          ?.filter((position: any) => {
            if (showOnlyActivePositions) {
              return position.unrealized !== 0;
            } else {
              return true;
            }
          })
          .sort(sorting(sortBy))
      );
    }
  }, [data, sortBy, isLoading, showOnlyActivePositions]);

  return (
    <>
      <Card className="grid grid-cols-1 gap-2">
        <Flex className="justify-between items-baseline truncate">
          <Text>Performance</Text>
          <Text>Total alpha: {formatCurrency(data?.alpha)}</Text>
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
              onValueChange={setSortBy as any}
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
              {positions?.map((item: any) => (
                <TableRow key={item.key}>
                  <TableCell>
                    <a
                      href={`https://duckduckgo.com/?q=${item.isin}`}
                      target="_blank"
                    >
                      {item.name.length > 12
                        ? `${item.name.substring(0, 12)}...`
                        : item.name}
                    </a>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.total_return === 0
                          ? "gray"
                          : item.total_return < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatRelativeAmount(item.total_return)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.realized === 0
                          ? "gray"
                          : item.realized < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.realized)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.unrealized === 0
                          ? "gray"
                          : item.unrealized < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.unrealized)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.alpha === 0
                          ? "gray"
                          : item.alpha < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.alpha)}
                    </Text>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Card>
    </>
  );
};

export default Performance;
