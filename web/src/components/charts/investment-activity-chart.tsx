import { useState } from "react"
import useSWR from "swr"
import { Bar, BarChart, CartesianGrid, ReferenceLine, XAxis, YAxis } from "recharts"

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import {
  ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart"
import { Skeleton } from "@/components/ui/skeleton"
import { BASE_URL, fetcher } from "@/lib/http"
import { formatCurrency } from "@/lib/utils"
import { MonthlyInvestmentActivity } from "@/types/core"

const chartConfig = {
  net_purchases_eur: {
    label: "Net purchases",
    color: "var(--chart-1)",
  },
  net_cash_added_eur: {
    label: "Net cash added",
    color: "var(--chart-2)",
  },
  dividends_interest_eur: {
    label: "Dividends & interest",
    color: "var(--chart-3)",
  },
} satisfies ChartConfig

const chartSeries = Object.entries(chartConfig)
const shortMonthFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  year: "2-digit",
  timeZone: "UTC",
})
const fullMonthFormat = new Intl.DateTimeFormat(undefined, {
  month: "long",
  year: "numeric",
  timeZone: "UTC",
})
const compactAmountFormat = new Intl.NumberFormat(undefined, {
  notation: "compact",
  maximumFractionDigits: 1,
})

const monthDate = (value: string) => {
  const [year, month] = value.split("-")
  return new Date(Date.UTC(Number(year), Number(month) - 1, 1))
}

const InvestmentActivityChart = ({ startDate }: { startDate: string }) => {
  const [tooltipPortal, setTooltipPortal] = useState<HTMLDivElement | null>(null)
  const { data, isLoading, error } = useSWR<MonthlyInvestmentActivity[]>(
    `${BASE_URL}/timeline/investment-activity?start_date=${startDate}`,
    fetcher,
  )

  if (isLoading) {
    return (
      <Card className="mb-6 min-w-0">
        <CardHeader>
          <CardTitle>Investment activity</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-80" />
        </CardContent>
      </Card>
    )
  }
  if (error) {
    return (
      <Card className="mb-6 min-w-0">
        <CardHeader>
          <CardTitle>Investment activity</CardTitle>
        </CardHeader>
        <CardContent>
          <p role="alert" className="text-sm text-destructive">
            Investment activity could not be loaded.
          </p>
        </CardContent>
      </Card>
    )
  }
  if (!data?.length) return null

  const chartData = data.map((item) => ({
    month: item.month,
    net_purchases_eur: parseFloat(item.net_purchases_eur),
    net_cash_added_eur: parseFloat(item.net_cash_added_eur),
    dividends_interest_eur: parseFloat(item.dividends_interest_eur),
  }))

  return (
    <Card className="mb-6 min-w-0">
      <CardHeader>
        <CardTitle>Investment activity</CardTitle>
        <CardDescription>
          Monthly EUR: net purchases, net cash added, and gross dividends &amp; interest before withholding tax.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="relative min-w-0">
          {/* Keep the tooltip visible while the plot scrolls horizontally. */}
          <div
            ref={setTooltipPortal}
            className="pointer-events-none absolute inset-x-0 top-2 z-10 flex justify-center"
          />
          <div
            className="overflow-x-auto scheme-light dark:scheme-dark"
            role="region"
            aria-label="Monthly investment activity chart; scroll horizontally for more months"
            tabIndex={0}
          >
            <ChartContainer
              config={chartConfig}
              className="aspect-auto h-80 min-w-0 w-full"
              style={{ minWidth: chartData.length * 64 + 80 }}
            >
              <BarChart
                accessibilityLayer
                data={chartData}
                margin={{ top: 12, left: 0, right: 12, bottom: 4 }}
                barGap={3}
                barCategoryGap="20%"
                maxBarSize={20}
              >
                <CartesianGrid vertical={false} strokeDasharray="3 3" />
                <XAxis
                  dataKey="month"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  minTickGap={16}
                  tickFormatter={(value) => shortMonthFormat.format(monthDate(value))}
                />
                <YAxis
                  width={68}
                  tickLine={false}
                  axisLine={false}
                  tickMargin={6}
                  tickFormatter={(value) => compactAmountFormat.format(Number(value))}
                  label={{
                    value: "EUR",
                    angle: -90,
                    position: "insideLeft",
                    offset: 10,
                    className: "fill-muted-foreground",
                  }}
                />
                <ChartTooltip
                  cursor={false}
                  shared
                  filterNull={false}
                  portal={tooltipPortal}
                  wrapperStyle={{ pointerEvents: "none", maxWidth: "100%" }}
                  content={
                    <ChartTooltipContent
                      className="w-64 max-w-full"
                      labelFormatter={(label) => fullMonthFormat.format(monthDate(String(label)))}
                      formatter={(value, name) => {
                        const series = chartConfig[name as keyof typeof chartConfig]
                        return (
                          <div className="grid w-full min-w-0 grid-cols-[0.625rem_minmax(0,1fr)_auto] items-center gap-2">
                            <span
                              className="h-2.5 w-2.5 shrink-0 rounded-[2px]"
                              style={{ backgroundColor: series.color }}
                            />
                            <span className="min-w-0 text-muted-foreground">{series.label}</span>
                            <span className="whitespace-nowrap font-mono font-medium tabular-nums text-foreground">
                              {formatCurrency(Number(value))}
                            </span>
                          </div>
                        )
                      }}
                    />
                  }
                />
                {chartSeries.map(([key]) => (
                  <Bar
                    key={key}
                    dataKey={key}
                    fill={`var(--color-${key})`}
                    radius={3}
                    minPointSize={(value) => value ? 2 : 0}
                  />
                ))}
                <ReferenceLine y={0} stroke="var(--foreground)" strokeOpacity={0.6} />
              </BarChart>
            </ChartContainer>
          </div>
        </div>
        <ul className="mt-4 flex flex-wrap justify-center gap-x-4 gap-y-2 text-xs">
          {chartSeries.map(([key, series]) => (
            <li key={key} className="flex items-center gap-1.5">
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-[2px]"
                style={{ backgroundColor: series.color }}
              />
              {series.label}
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  )
}

export default InvestmentActivityChart
