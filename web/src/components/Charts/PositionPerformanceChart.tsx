import React, { useEffect, useState } from "react";
import {
  ScatterChart,
  Scatter,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  Cell,
} from "recharts";
import useSwr from "swr";
import { BASE_URL, formatCurrency, fetcher } from "@/util";
import { PortfolioPerformance } from "@/types/core";

interface ChartDataItem {
  total_return: number;
  invested_amount: number;
  name: string;
}

const PositionPerformanceScatterChart: React.FC = () => {
  const { data, isLoading } = useSwr<PortfolioPerformance>(
    `${BASE_URL}/performance_overview`,
    fetcher
  );
  const [chartData, setChartData] = useState<ChartDataItem[]>([]);

  useEffect(() => {
    if (!isLoading && data) {
      const mappedData = data.position.map(({ total_return, invested_amount, name }) => ({
        total_return,
        invested_amount,
        name,
      }));
      setChartData(mappedData);
    }
  }, [data, isLoading]);

  const largestInvestedAmount = Math.max(...chartData.map(d => d.invested_amount), 0);
  const largestReturn = Math.max(...chartData.map(d => d.total_return), 0);

  const CustomTooltip = ({ payload, active }: any) => {
    if (active && payload?.length) {
      const { name, total_return, invested_amount } = payload[0].payload;
      return (
        <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-800 shadow rounded-lg p-4 text-sm">
          <p className="font-bold text-gray-700 dark:text-white">{name}</p>
          <p className="text-gray-500">{`${total_return}% Total Return`}</p>
          <p className="text-gray-500">{`Invested: ${formatCurrency(invested_amount)}`}</p>
        </div>
      );
    }
    return null;
  };

  if (isLoading) {
    return <div className="h-40" />;
  }

  return (
    <ResponsiveContainer width="100%" height={400}>
      <ScatterChart className="mt-4">
        <CartesianGrid className="stroke-gray-200 dark:stroke-gray-800" />
        <XAxis
          type="number"
          dataKey="total_return"
          name="Total Return"
          unit="%"
          label={{ fontSize: 14 }}
          tick={{ fontSize: 14 }}
          domain={[-100, largestReturn]}
        />
        <YAxis
          type="number"
          dataKey="invested_amount"
          name="Invested Amount"
          unit="EUR"
          hide
          domain={[0, largestInvestedAmount]}
        />
        <Tooltip content={<CustomTooltip />} />
        <Scatter data={chartData}>
          {chartData.map((entry, index) => (
            <Cell
              key={`cell-${index}`}
              className={entry.total_return > 0 ? "fill-green-500" : "fill-red-500"}
            />
          ))}
        </Scatter>
      </ScatterChart>
    </ResponsiveContainer>
  );
};

export default PositionPerformanceScatterChart;

