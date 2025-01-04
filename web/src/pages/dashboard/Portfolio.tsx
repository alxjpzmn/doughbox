import {
  Card,
  Flex,
  Metric,
  BarList,
  Text,
  BadgeDelta,
  Subtitle,
  Divider,
  Bold,
} from '@tremor/react';
import useSwr from 'swr';
import {
  BASE_URL,
  colors,
  formatCurrency,
  fetcher,
  formatUnixTimestampRelative,
  getDeltaType,
} from '@/util';
import { PortfolioOverview, PositionWithValueAndAllocation } from '@/types/core';
import EmptyState from '@/components/EmptyState';


const Portfolio = () => {
  const { data, isLoading } = useSwr<PortfolioOverview>(`${BASE_URL}/portfolio`, fetcher);

  let overviewData = {
    title: 'Current Portfolio Value',
    metric: `${formatCurrency(isLoading || !data ? 0 : parseFloat(data.total_value))}`,
    metricPrev: `${formatCurrency(
      isLoading || !data ? 0 : parseFloat(data?.total_return_abs)
    )}`,
    delta: `${isLoading || !data ? '0,00%' : `${data.total_return_rel}%`}`,
    deltaType: (isLoading || !data) ? 'unchanged' : getDeltaType(parseFloat(data?.total_return_abs)),
    updatedAt: (isLoading || !data) ? new Date() : data.generated_at,
  };

  return (
    <>
      <EmptyState />
      {data && <>
        <Card key={overviewData.title} className="mb-6">
          <Subtitle>{overviewData.title}</Subtitle>
          <Flex className="justify-start gap-3 items-baseline truncate">
            <Metric>{overviewData.metric}</Metric>
          </Flex>
          <Flex className="my-4">
            <BadgeDelta deltaType={overviewData.deltaType} className="mr-2" size="xs" />
            <Flex className="justify-between gap-4 truncate">
              <Text color={colors[overviewData.deltaType]}>{overviewData.delta}</Text>
              <Text className="truncate">
                <Bold>{overviewData.metricPrev}</Bold> total return
              </Text>
            </Flex>
          </Flex>
          <Divider />

          <Text>Last updated {formatUnixTimestampRelative(data?.generated_at)}</Text>
        </Card>

        <Card>
          <Text>Portfolio</Text>
          <BarList
            data={data?.positions
              .map((position: PositionWithValueAndAllocation) => {
                return {
                  name: `${position.share}% · ${position.name}`,
                  value: parseFloat(position.value),
                  href: `https://duckduckgo.com/?q=${position.isin}`,
                };
              })}
            className="mt-4"
            valueFormatter={formatCurrency}
          />
        </Card></>}
    </>
  );
};

export default Portfolio;
