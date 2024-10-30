import React, { useEffect, useState } from "react";
import {
  BASE_URL,
  formatCurrency,
  fetcher,
  formatUnixTimestampRelative,
  formatRelativeAmount,
} from "../../util";
import useSwr from "swr";
import PerformanceChart from "./PerformanceChart";
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
import PositionPerformanceScatterChart from "./PositionPerformanceScatterChart";
import { Switch } from "../../components/Switch";
import { Label } from "../../components/Label";

interface PLProps { }

type sortByMethods =
  | "ascRoe"
  | "descRoe"
  | "ascRealized"
  | "descRealized"
  | "ascUnrealized"
  | "descUnrealized"
  | "ascAlpha"
  | "descAlpha";

const PL: React.FC<PLProps> = ({ }) => {
  const { data, isLoading } = useSwr(`${BASE_URL}/pl`, fetcher);

  const sorting = (method: sortByMethods) => {
    switch (method) {
      case "ascRoe":
        return (a: any, b: any) => a.return_on_equity - b.return_on_equity;
      case "descRoe":
        return (a: any, b: any) => b.return_on_equity - a.return_on_equity;
      case "ascRealized":
        return (a: any, b: any) => a.realized_pl - b.realized_pl;
      case "descRealized":
        return (a: any, b: any) => b.realized_pl - a.realized_pl;
      case "ascUnrealized":
        return (a: any, b: any) => a.unrealized_pl - b.unrealized_pl;
      case "descUnrealized":
        return (a: any, b: any) => b.unrealized_pl - a.unrealized_pl;
      case "ascAlpha":
        return (a: any, b: any) => a.real_vs_sim - b.real_vs_sim;
      case "descAlpha":
        return (a: any, b: any) => b.real_vs_sim - a.real_vs_sim;
      default:
        return null;
    }
  };

  const [sortBy, setSortBy] = useState<sortByMethods>("ascRoe");
  const [showOnlyActivePositions, setShowOnlyActivePositions] = useState(false);
  const [positions, setPositions] = useState([]);

  useEffect(() => {
    if (!isLoading) {
      setPositions(
        data?.position_pl
          ?.filter((item: any) => {
            if (showOnlyActivePositions) {
              return item.unrealized_pl !== "0";
            } else {
              return true;
            }
          })
          .sort(sorting(sortBy))
          .map((position: any) => {
            return {
              key: `${position.isin}-${position.return_on_equity}`,
              ...position,
            };
          }),
      );
    }
  }, [data, sortBy, isLoading, showOnlyActivePositions]);

  return (
    <>
      <Card className="grid grid-cols-1 gap-2">
        <Flex className="justify-between items-baseline truncate">
          <Text>Performance</Text>
          <Text>Total alpha: {formatCurrency(data?.total_alpha)}</Text>
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
          <Text className="flex-grow">Individiual P&L</Text>
          <Flex className="flex md:justify-between">
            <Select
              value={undefined}
              defaultValue={sortBy}
              onValueChange={setSortBy as any}
              placeholder="Sort by..."
              className="max-w-min mt-0"
            >
              <SelectItem value="ascRoe">RoE Ascending</SelectItem>
              <SelectItem value="descRoe">RoE Descending</SelectItem>
              <SelectItem value="ascRealized">
                Realized P/L Ascending
              </SelectItem>
              <SelectItem value="descRealized">
                Realized P/L Descending
              </SelectItem>
              <SelectItem value="ascUnrealized">
                Unrealized P/L Ascending
              </SelectItem>
              <SelectItem value="descUnrealized">
                Unrealized P/L Descending
              </SelectItem>
              <SelectItem value="ascAlpha">Alpha Ascending</SelectItem>
              <SelectItem value="descAlpha">Alpha Descending</SelectItem>
            </Select>
            <div className="flex items-center justify-center gap-2">
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
                <TableHeaderCell>RoE</TableHeaderCell>
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
                        item.return_on_equity === 0
                          ? "gray"
                          : item.return_on_equity < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatRelativeAmount(item.return_on_equity)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.realized_pl === 0
                          ? "gray"
                          : item.realized_pl < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.realized_pl)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.unrealized_pl === 0
                          ? "gray"
                          : item.unrealized_pl < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.unrealized_pl)}
                    </Text>
                  </TableCell>
                  <TableCell>
                    <Text
                      color={
                        item.real_vs_sim === 0
                          ? "gray"
                          : item.real_vs_sim < 0
                            ? "red"
                            : "green"
                      }
                    >
                      {formatCurrency(item.real_vs_sim)}
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

export default PL;
