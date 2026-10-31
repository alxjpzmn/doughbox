import React from "react";
import useSwr from "swr";
import { PerformanceSignal } from "@/types/core";
import { Skeleton } from "@/components/ui/skeleton";
import { ChartConfig, ChartContainer, ChartTooltip, ChartTooltipContent } from "@/components/ui/chart";
import { CartesianGrid, XAxis, Area, AreaChart } from "recharts";
import { BASE_URL, fetcher } from "@/lib/http";
import { formatCurrency, formatDate } from "@/lib/utils";
import { Button } from "@/components/ui/button";

export type HistoryMeasure = 'amount' | 'percentage';

interface PerformanceChartProps {
  activeOnly: boolean;
  activeAssetIds: ReadonlySet<string>;
  measure: HistoryMeasure;
  onMeasureChange: (measure: HistoryMeasure) => void;
}

const PerformanceChart: React.FC<PerformanceChartProps> = ({ activeOnly, activeAssetIds, measure, onMeasureChange }) => {
  const { data, error, isLoading } = useSwr<PerformanceSignal[]>(`${BASE_URL}/past_performance`, fetcher);

  if (error) {
    return (
      <p role="alert" className="py-8 text-sm text-destructive">
        Return history could not be loaded.
      </p>
    );
  }

  if (activeOnly && activeAssetIds.size === 0) {
    return <p className="py-8 text-sm text-muted-foreground">No active holdings to display in return history.</p>;
  }

  const chartData = data?.map((signal) => {
    const missingBreakdown = signal.components == null;
    let totalValue = Number(signal.total_value);
    let totalInvested = Number(signal.total_invested);
    let grossInvested = signal.gross_invested == null ? NaN : Number(signal.gross_invested);
    if (activeOnly) {
      totalValue = 0;
      totalInvested = 0;
      grossInvested = 0;
      for (const component of signal.components ?? []) {
        if (!activeAssetIds.has(component.asset_id)) continue;
        totalValue += Number(component.total_value);
        totalInvested += Number(component.total_invested);
        grossInvested += component.gross_invested == null ? NaN : Number(component.gross_invested);
      }
    }
    const totalsAvailable = (!activeOnly || !missingBreakdown)
      && Number.isFinite(totalValue) && Number.isFinite(totalInvested);
    const missingGross = !Number.isFinite(grossInvested) || grossInvested <= 0;
    const value = !totalsAvailable || (measure === 'percentage' && missingGross)
      ? null
      : measure === 'amount' ? totalValue - totalInvested
        : ((totalValue - totalInvested) / grossInvested) * 100;
    return {
      date: new Date(signal.date).getTime(),
      current: !missingBreakdown ? value : null,
      legacy: !activeOnly && missingBreakdown ? value : null,
      missingBreakdown,
      missingGross,
    };
  });
  const currentCount = chartData?.filter(point => point.current != null).length ?? 0;
  const legacyCount = chartData?.filter(point => point.legacy != null).length ?? 0;
  const hasValues = currentCount > 0 || legacyCount > 0;
  const hasMissingBreakdowns = chartData?.some(point => point.missingBreakdown);
  const hasGaps = chartData?.some(point => point.current == null && point.legacy == null);
  const latestValue = chartData?.[chartData.length - 1]?.current;
  const chartConfig = {
    current: {
      label: 'P&L including recorded income',
      color: `var(${latestValue != null && latestValue < 0 ? '--destructive-foreground' : '--success-foreground'})`,
    },
    legacy: {
      label: 'Legacy snapshot P&L',
      color: 'var(--muted-foreground)',
    },
  } satisfies ChartConfig;

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div role="group" aria-label="History measure" className="flex gap-1">
          <Button type="button" variant={measure === 'amount' ? 'secondary' : 'ghost'} size="sm"
            aria-pressed={measure === 'amount'} onClick={() => onMeasureChange('amount')}>EUR P&amp;L</Button>
          <Button type="button" variant={measure === 'percentage' ? 'secondary' : 'ghost'} size="sm"
            aria-pressed={measure === 'percentage'} onClick={() => onMeasureChange('percentage')}>Return %</Button>
        </div>
        {hasValues && <ul aria-label="History series" className="flex flex-wrap gap-4 text-xs text-muted-foreground">
          {currentCount > 0 && <li className="flex items-center gap-2">
            <span aria-hidden="true" className="w-5 border-t-2" style={{ borderColor: chartConfig.current.color }} />Current
          </li>}
          {legacyCount > 0 && <li className="flex items-center gap-2">
            <span aria-hidden="true" className="w-5 border-t-2 border-dashed border-muted-foreground" />Legacy
          </li>}
        </ul>}
      </div>
      {!isLoading && hasValues ? (
        <ChartContainer config={chartConfig} className="h-64 w-full aspect-auto">
          <AreaChart accessibilityLayer data={chartData} margin={{ top: 8, left: 12, right: 12 }}>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="date"
              type="number"
              domain={['dataMin', 'dataMax']}
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              tickFormatter={(value) => new Date(value).toLocaleDateString(undefined, { month: 'short', year: '2-digit' })}
            />
            <ChartTooltip
              cursor={false}
              content={
                <ChartTooltipContent
                  labelFormatter={(_label, payload) => formatDate(new Date(payload[0].payload.date))}
                  formatter={(value, _name, item) => (
                    <div className="grid gap-1">
                      <span className="text-muted-foreground">{item.dataKey === 'legacy' ? 'Legacy snapshot P&L' : 'P&L including recorded income'}</span>
                      <span className="font-mono font-medium">{measure === 'amount' ? formatCurrency(Number(value)) : `${Number(value).toFixed(2)}%`}</span>
                      {item.dataKey === 'legacy' && <span className="text-muted-foreground">As originally recorded; income may be incomplete.</span>}
                    </div>
                  )}
                />
              }
            />
            <Area
              dataKey="legacy"
              type="linear"
              connectNulls={false}
              fill="none"
              stroke="var(--color-legacy)"
              strokeDasharray="5 4"
              strokeWidth={2}
              strokeLinecap="round"
              dot={legacyCount === 1 ? { r: 3 } : false}
              isAnimationActive={false}
            />
            <Area
              dataKey="current"
              type="linear"
              connectNulls={false}
              fill="none"
              stroke="var(--color-current)"
              strokeWidth={2}
              strokeLinecap="round"
              dot={currentCount === 1 ? { r: 3 } : false}
              isAnimationActive={false}
            />
          </AreaChart>
        </ChartContainer>
      ) : isLoading ? (
        <Skeleton className="h-64" />
      ) : !data?.length ? (
        <p className="py-8 text-sm text-muted-foreground">No return history snapshots have been recorded.</p>
      ) : (
        <p className="py-8 text-sm text-muted-foreground">
          {activeOnly && hasMissingBreakdowns
            ? "Older aggregate snapshots cannot be filtered to today's active holdings."
            : "Percentage history needs a recorded positive gross purchase or contribution baseline. Choose EUR P&L to view older snapshots."}
        </p>
      )}
      {!isLoading && !activeOnly && hasMissingBreakdowns && (
        <p className="mt-3 text-xs text-muted-foreground">Dashed legacy snapshots retain their original P&amp;L. Income has not been backfilled, and legacy and current series are not joined.</p>
      )}
      {!isLoading && hasValues && hasGaps && (
        <p className="mt-3 text-xs text-muted-foreground">
          {activeOnly ? "Gaps lack a per-asset breakdown or a positive gross baseline for percentage returns. Full-portfolio values are never substituted." : measure === 'percentage' ? "Gaps lack a recorded positive gross purchase baseline; no net-capital denominator is substituted." : "Gaps have unavailable saved totals."}
        </p>
      )}
    </>
  );
};

export default PerformanceChart;
