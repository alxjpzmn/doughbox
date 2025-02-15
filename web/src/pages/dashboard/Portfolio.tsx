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
import EmptyState, { EmptyStateVariants } from '@/components/EmptyState';
import { Skeleton } from '@/components/Skeleton';
import { Disclaimer } from '@/components/Disclaimer';

const Portfolio = () => {
  const { data, isLoading, error } = useSwr<PortfolioOverview>(`${BASE_URL}/portfolio`, fetcher);

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
      {error && !error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionImportTrades} docker={error.details?.in_docker} />}
      {isLoading ? (
        <>
          <Card className="mb-6">
            <Skeleton className="h-6 w-1/4 mb-4" />
            <Skeleton className="h-12 w-1/2 mb-4" />
            <Flex className="my-4">
              <Skeleton className="h-6 w-16 mr-2" />
              <Flex className="justify-between gap-4 truncate">
                <Skeleton className="h-6 w-1/4" />
                <Skeleton className="h-6 w-1/2" />
              </Flex>
            </Flex>
            <Divider />
            <Skeleton className="h-6 w-1/3" />
          </Card>
          <Card>
            {Array.from({ length: 20 }).map((_, index) =>
              <div className='flex gap-4'>
                <Skeleton key={index} className="h-6 w-3/4 mb-4" />
                <Skeleton key={index} className="h-6 w-1/4 mb-4" />
              </div>
            )}
          </Card>
        </>
      ) : data && (
        <>
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

          {data?.positions.length > 0 && (
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
            </Card>
          )}
          <Disclaimer />
        </>
      )}
    </>
  );
};

export default Portfolio;
