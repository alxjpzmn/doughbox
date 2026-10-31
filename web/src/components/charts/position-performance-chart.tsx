import React from "react";
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
import { PositionPerformance } from "@/types/core";
import { formatCurrency } from "@/lib/utils";

interface ChartDataItem {
  total_return: number;
  invested_amount: number;
  name: string;
  isin: string;
}

interface PositionPerformanceChartProps {
  positions: PositionPerformance[];
}

const PositionPerformanceChart: React.FC<PositionPerformanceChartProps> = ({ positions }) => {
  const chartData: ChartDataItem[] = positions.map(({ total_return, invested_amount, name, isin }) => ({
    total_return: Number(total_return),
    invested_amount: Number(invested_amount),
    name,
    isin,
  })).filter((entry) => Number.isFinite(entry.invested_amount) && entry.invested_amount > 0 && Number.isFinite(entry.total_return));
  const largestInvestedAmount = Math.max(...chartData.map((entry) => entry.invested_amount), 0);
  const largestReturn = Math.max(...chartData.map((entry) => entry.total_return), 0);
  const smallestReturn = Math.min(...chartData.map((entry) => entry.total_return), -100);

  if (chartData.length === 0) {
    return <p className="py-8 text-sm text-muted-foreground">{positions.length === 0 ? "No securities performance for the selected holdings. Manual holdings appear in P&L history only." : "Percentage returns are unavailable without a recorded positive gross purchase baseline. Recorded income is still shown in the individual table."}</p>;
  }

  return (
    <>
    <ResponsiveContainer width="100%" height={400}>
      <ScatterChart className="mt-4">
        <CartesianGrid className="stroke-muted" />
        <XAxis
          type="number"
          dataKey="total_return"
          name="Return incl. recorded income"
          unit="%"
          label={{ fontSize: 14 }}
          tick={{ fontSize: 14 }}
          domain={[smallestReturn, largestReturn]}
        />
        <YAxis
          type="number"
          dataKey="invested_amount"
          name="Gross Purchases"
          unit="EUR"
          hide
          domain={[0, largestInvestedAmount]}
        />
        <Tooltip content={({ payload, active }) => {
          if (!active || !payload?.length) return null;
          const { name, total_return, invested_amount } = payload[0].payload as ChartDataItem;
          return (
            <div className="bg-muted border rounded p-4 text-sm">
              <p className="font-bold">{name}</p>
              <p className="text-muted-foreground">{total_return.toFixed(2)}% Return incl. recorded gross dividends</p>
              <p className="text-muted-foreground">{formatCurrency(invested_amount)} Gross Purchases</p>
            </div>
          );
        }} />
        <Scatter data={chartData}>
          {chartData.map((entry) => (
            <Cell
              key={entry.isin}
              className={entry.total_return > 0 ? "fill-success-foreground" : "fill-destructive-foreground"}
            />
          ))}
        </Scatter>
      </ScatterChart>
    </ResponsiveContainer>
      {chartData.length < positions.length && <p className="mt-3 text-xs text-muted-foreground">Holdings without a recorded positive gross purchase baseline have no percentage return and are omitted from this chart; their recorded income remains in the individual table.</p>}
    </>
  );
};

export default PositionPerformanceChart;
