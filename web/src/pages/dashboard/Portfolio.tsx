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


const Portfolio = () => {
  const { data, isLoading } = useSwr(`${BASE_URL}/portfolio`, fetcher);

  let overviewData = {
    title: 'Current Portfolio Value',
    metric: `${formatCurrency(isLoading ? 0 : (data as any)?.total_value)}`,
    metricPrev: `${formatCurrency(
      isLoading ? 0 : (data as any)?.total_roe_abs
    )}`,
    delta: `${isLoading ? '0,00%' : `${(data as any)?.total_roe_rel}%`}`,
    deltaType: getDeltaType((data as any)?.total_roe_rel),
    updatedAt: (data as any)?.generated_at,
  };

  return (
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

      <Card>
        <Text>Portfolio</Text>
        <BarList
          data={data?.positions
            ?.filter((position: any) => {
              return position.current_value > 0 ? true : false;
            })
            .map((position: any) => {
              return {
                name: `${position.share.replace('.', ',')}% · ${position.name}`,
                value: position.current_value,
                href: `https://duckduckgo.com/?q=${position.isin}`,
              };
            })
            .sort((a: any, b: any) => a.value < b.value)}
          className="mt-4"
          valueFormatter={formatCurrency}
        />
      </Card>
    </>
  );
};

export default Portfolio;
