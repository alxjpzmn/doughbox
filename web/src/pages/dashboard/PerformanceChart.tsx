import { AreaChart } from "@tremor/react";
import React from "react";
import useSwr from "swr";
import { BASE_URL, fetcher, formatDate, formatRelativeAmount } from "../../util";

interface PerformanceChartProps { }

const PerformanceChart: React.FC<PerformanceChartProps> = ({ }) => {
  const { data, isLoading } = useSwr(`${BASE_URL}/performance`, fetcher);

  const relativeReturnChartData = data?.map((performanceSignal: any) => {
    return {
      date: formatDate(new Date(performanceSignal?.date)),
      "Relative Return":
        ((performanceSignal?.total_value - performanceSignal?.total_invested) /
          performanceSignal?.total_invested) *
        100,
    };
  });

  return (
    <>
      {!isLoading && !!relativeReturnChartData ? (
        <AreaChart
          className="mt-4 h-64"
          data={relativeReturnChartData}
          index="date"
          valueFormatter={formatRelativeAmount}
          categories={["Relative Return"]}
          colors={
            !!relativeReturnChartData?.length
              ? relativeReturnChartData[relativeReturnChartData?.length - 1][
                "Relative Return"
              ] > 0
                ? ["green"]
                : ["red"]
              : ["green"]
          }
          showXAxis={false}
          showGridLines={true}
          startEndOnly={true}
          showYAxis={false}
          showLegend={false}
        />
      ) : (
        <div className="h-64 bg-slate-200 rounded mt-4 animate-pulse" />
      )}
    </>
  );
};

export default PerformanceChart;
