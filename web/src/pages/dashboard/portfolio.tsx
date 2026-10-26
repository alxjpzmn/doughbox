import useSwr from 'swr';
import { PortfolioOverview, PositionWithValueAndAllocation } from '@/types/core';
import EmptyState, { EmptyStateVariants } from '@/components/composite/empty-state';
import { Skeleton } from '@/components/ui/skeleton';
import { Disclaimer } from '@/components/composite/disclaimer';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { BarList } from '@/components/composite/bar-list';
import { BASE_URL, fetcher } from '@/lib/http';
import { formatCurrency, formatRelativeAmount } from '@/lib/utils';
import { ArrowUpIcon, ArrowDownIcon, MinusIcon } from '@phosphor-icons/react';

const Portfolio = () => {
  const { data, isLoading, error } = useSwr<PortfolioOverview>(`${BASE_URL}/portfolio`, fetcher, { refreshInterval: 60_000 });

  let overviewData = {
    title: data && !data.valuation_complete ? 'Partial Portfolio Value' : 'Current Portfolio Value',
    portfolio_value: `${formatCurrency(isLoading || !data ? 0 : parseFloat(data.total_value))}`,
    absolute_return: `${formatCurrency(
      isLoading || !data ? 0 : parseFloat(data?.total_return_abs)
    )}`,
    relative_return: `${isLoading || !data ? formatRelativeAmount(0) : formatRelativeAmount(parseFloat(data.total_return_rel))}`,
    unformatted_return:
      isLoading || !data ? 0 : parseFloat(data?.total_return_abs)
  };


  return (
    <>
      {error && !error.details.events_present && <EmptyState variant={EmptyStateVariants.WithCliInstructionImportTrades} docker={error.details?.in_docker} />}
      {isLoading ? (
        <>
          {/* Skeleton for Portfolio Overview Card */}
          <Card className="mb-6">
            <CardHeader>
              <CardTitle>
                <Skeleton className="h-6 w-1/4" /> {/* Placeholder for "Current Portfolio Value" */}
              </CardTitle>
              <CardDescription>
                <Skeleton className="h-4 w-1/2" /> {/* Placeholder for "Last updated" */}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Skeleton className="h-8 w-1/2 mb-2" /> {/* Placeholder for Portfolio Value */}
              <Skeleton className="h-4 w-1/3 mb-0.5" /> {/* Placeholder for Additional Info */}
            </CardContent>
          </Card>

          {/* Skeleton for Portfolio Positions Card */}
          <Card>
            <CardHeader>
              <CardTitle>
                <Skeleton className="h-6 w-1/4" /> {/* Placeholder for "Portfolio" title */}
              </CardTitle>
            </CardHeader>
            <CardContent>
              {Array.from({ length: 20 }).map((_, index) => (
                <div key={index} className="flex gap-6 mt-2 mb-4">
                  <Skeleton className="h-8 w-3/4" /> {/* Placeholder for Position Name */}
                  <Skeleton className="h-8 w-1/4" /> {/* Placeholder for Position Value */}
                </div>
              ))}
            </CardContent>
          </Card>
        </>
      ) : data && (
        <>
          {/* Portfolio Overview Card */}
          <Card key={overviewData.title} className="mb-6">
            <CardHeader>
              <CardTitle>{overviewData.title}</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex justify-start mb-2 truncate">
                <p className='text-4xl font-bold'>{overviewData.portfolio_value}</p>
              </div>
              {data.valuation_complete ? <div className="text-muted-foreground flex gap-2 items-center leading-none text-sm truncate">
                {
                  overviewData.unformatted_return > 0 && <ArrowUpIcon size={16} className='text-success-foreground' />
                }
                {
                  overviewData.unformatted_return < 0 && <ArrowDownIcon size={16} className='text-destructive-foreground' />
                }
                {
                  overviewData.unformatted_return === 0 && <MinusIcon size={16} className='text-muted-foreground' />
                }
                <p>{overviewData.absolute_return} (
                  {overviewData.relative_return}
                  )</p>
              </div> : <p className="text-sm text-destructive-foreground">
                Excludes holdings without a usable price: {data.unpriced_isins.join(', ')}. Returns are unavailable until all holdings are priced.
              </p>}
            </CardContent>
          </Card>

          {/* Portfolio Positions Card */}
          {data?.positions.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle>
                  Portfolio
                </CardTitle>
              </CardHeader>
              <CardContent>
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
              </CardContent>
            </Card>
          )}
          <Disclaimer />
        </>
      )}
    </>
  );
};

export default Portfolio;
