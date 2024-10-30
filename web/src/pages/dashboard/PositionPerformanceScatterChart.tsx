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
import { BASE_URL, formatCurrency, fetcher } from "../../util";

interface PositionPerformanceScatterChartProps { }

const PositionPerformanceScatterChart: React.FC<
  PositionPerformanceScatterChartProps
> = ({ }) => {
  const { data, isLoading } = useSwr(`${BASE_URL}/pl`, fetcher);
  const [chartData, setChartData] = useState([]);

  useEffect(() => {
    if (!isLoading) {
      setChartData(
        data?.position_pl?.map((dataPoint: any) => {
          return {
            roe: dataPoint.return_on_equity,
            invested_amount: dataPoint.invested_amount,
            name: dataPoint.name,
          };
        }),
      );
    }
  }, [data, isLoading]);

  const [largestInvestedAmount, setLargestInvestedAmount] = useState(0);

  useEffect(() => {
    const largestInvestedAmountItem = (chartData as any)?.map((item: any) => item).sort((a: any, b: any) => parseFloat(a.invested_amount) > parseFloat(b.invested_amount)).pop();
    setLargestInvestedAmount(parseFloat(largestInvestedAmountItem?.invested_amount));
  }, [chartData])

  const [largestRoe, setLargestRoe] = useState(0);
  useEffect(() => {
    const largestRoeItem = (chartData as any)?.map((item: any) => item).sort((a: any, b: any) => parseFloat(a.roe) > parseFloat(b.roe)).pop();
    setLargestRoe(parseFloat(largestRoeItem?.roe));
  }, [chartData])



  //@ts-ignore
  const CustomTooltip = ({ payload, active }) => {
    if (active) {
      return (
        <div className="bg-white dark:bg-gray-900 outline-current border border-gray-200 dark:border-gray-800 shadow rounded-lg p-4 text-sm ring-0">
          <p className="font-bold text-gray-700 dark:text-white">{`${payload[0].payload.name}`}</p>
          <p className="text-gray-500">{`${payload[0].payload.roe}% RoE `}</p>
          <p className="text-gray-500">{`Invested: ${formatCurrency(
            payload[0].payload.invested_amount,
          )}`}</p>
        </div>
      );
    }

    return null;
  };

  return (
    <>
      {!isLoading ? (
        <ResponsiveContainer width="100%" height={400}>
          <ScatterChart className="mt-4">
            <CartesianGrid className="stroke-gray-200 dark:stroke-gray-800" />
            <XAxis
              type="number"
              dataKey="roe"
              name="Return"
              unit="%"
              label={{ fontSize: 14 }}
              tick={{ fontSize: 14 }}
              domain={[-100, largestRoe]}
            />
            <YAxis
              type="number"
              dataKey="invested_amount"
              name="Invested amount"
              unit="EUR"
              label={{ fontSize: 14 }}
              tick={{ fontSize: 14 }}
              hide={true}
              domain={[0, largestInvestedAmount]}
            />
            <Tooltip
              //@ts-ignore
              content={<CustomTooltip />}
            />
            <Scatter name="Return Scatter" data={chartData}>
              {chartData?.map((entry: any, index: any) => (
                <Cell
                  key={`cell-${index}`}
                  className={`${entry.roe > 0 ? 'fill-green-500' : 'fill-red-500'}`}
                />
              ))}
            </Scatter>
          </ScatterChart>
        </ResponsiveContainer>
      ) : (
        <div className="h-40" />
      )}
    </>
  );
};

export default PositionPerformanceScatterChart;
