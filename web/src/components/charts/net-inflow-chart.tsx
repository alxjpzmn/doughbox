import useSWR from "swr"
import { Bar, BarChart, CartesianGrid, Cell, XAxis } from "recharts"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { ChartConfig, ChartContainer, ChartTooltip, ChartTooltipContent } from "@/components/ui/chart"
import { Skeleton } from "@/components/ui/skeleton"
import { BASE_URL, fetcher } from "@/lib/http"
import { formatCurrency } from "@/lib/utils"
import { MonthlyNetInflow } from "@/types/core"

const chartConfig = {
  net_eur: {
    label: "Net inflow",
  },
} satisfies ChartConfig

const monthLabel = (value: string) => {
  const [year, month] = value.split("-")
  const date = new Date(Number(year), Number(month) - 1, 1)
  return new Intl.DateTimeFormat(undefined, { month: "short", year: "2-digit" }).format(date)
}

const NetInflowChart = () => {
  const { data, isLoading } = useSWR<MonthlyNetInflow[]>(`${BASE_URL}/timeline/net-inflow`, fetcher)

  if (isLoading) {
    return (
      <Card className="mb-6">
        <CardHeader>
          <CardTitle>Net inflow</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-64" />
        </CardContent>
      </Card>
    )
  }
  if (!data?.length) return null

  const chartData = data.map((item) => ({
    month: item.month,
    net_eur: parseFloat(item.net_eur),
  }))

  return (
    <Card className="mb-6">
      <CardHeader>
        <CardTitle>Net inflow</CardTitle>
      </CardHeader>
      <CardContent>
        <ChartContainer config={chartConfig} className="aspect-auto h-64 w-full">
          <BarChart accessibilityLayer data={chartData} margin={{ left: 12, right: 12 }}>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="month"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              minTickGap={24}
              tickFormatter={monthLabel}
            />
            <ChartTooltip
              cursor={false}
              content={
                <ChartTooltipContent
                  labelFormatter={(label) => monthLabel(String(label))}
                  formatter={(value) => formatCurrency(Number(value))}
                />
              }
            />
            <Bar dataKey="net_eur" radius={4}>
              {chartData.map((item) => (
                <Cell
                  key={item.month}
                  fill={`var(${item.net_eur >= 0 ? "--success-foreground" : "--destructive-foreground"})`}
                />
              ))}
            </Bar>
          </BarChart>
        </ChartContainer>
      </CardContent>
    </Card>
  )
}

export default NetInflowChart
