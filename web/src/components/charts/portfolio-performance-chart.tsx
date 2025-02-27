import React from "react";
import useSwr from "swr";
import { PerformanceSignal } from "@/types/core";
import { Skeleton } from "@/components/ui/skeleton";
import { ChartConfig, ChartContainer, ChartTooltip, ChartTooltipContent } from "@/components/ui/chart";
import { CartesianGrid, XAxis, Area, AreaChart } from "recharts";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatDate } from "@/lib/utils";

interface PerformanceChartProps { }

const PerformanceChart: React.FC<PerformanceChartProps> = ({ }) => {
  const { data, isLoading } = useSwr<PerformanceSignal[]>(`${BASE_URL}/past_performance`, fetcher);

  const relativeReturnChartData = data?.map((performanceSignal) => {
    return {
      date: formatDate(new Date(performanceSignal?.date)),
      "relative_return":
        ((parseFloat(performanceSignal?.total_value) - parseFloat(performanceSignal?.total_invested)) /
          parseFloat(performanceSignal?.total_invested)) *
        100,
    };
  });

  const chartConfig = {
    relative_return: {
      label: "Return",
    },
  } satisfies ChartConfig;

  return (
    <>
      {!isLoading && !!relativeReturnChartData ? (
        <ChartContainer config={chartConfig}>
          <AreaChart
            accessibilityLayer
            data={relativeReturnChartData}
            margin={{
              left: 12,
              right: 12,
            }}
          >
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="date"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              tickFormatter={(value) => value.slice(3, value.length)}
            />
            <ChartTooltip
              cursor={false}
              content={<ChartTooltipContent hideLabel />}
            />
            <Area
              dataKey="relative_return"
              type="linear"
              fill="none"
              stroke={`var(${relativeReturnChartData[relativeReturnChartData.length - 1].relative_return > 0 ? '--success-foreground' : '--destructive-foreground'})`}
              strokeWidth={2}
              strokeLinecap="round"
            />
          </AreaChart>
        </ChartContainer>
      ) : (
        <Skeleton className="h-64" />
      )}
    </>
  );
};

export default PerformanceChart;
