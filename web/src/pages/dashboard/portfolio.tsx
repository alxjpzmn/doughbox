import useSwr from 'swr';
import { PortfolioOverview, PositionWithValueAndAllocation } from '@/types/core';
import EmptyState, { EmptyStateVariants } from '@/components/composite/empty-state';
import { Skeleton } from '@/components/ui/skeleton';
import { Disclaimer } from '@/components/composite/disclaimer';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { BarList } from '@/components/composite/bar-list';
import { BASE_URL, fetcher } from '@/lib/http';
import { formatCurrency, formatRelativeAmount } from '@/lib/utils';
import { Triangle, TriangleAlert, TriangleDashed } from 'lucide-react';
import { Badge } from '@/components/ui/badge';

const Portfolio = () => {
  const { data, isLoading, error } = useSwr<PortfolioOverview>(`${BASE_URL}/portfolio`, fetcher);

  let overviewData = {
    title: 'Current Portfolio Value',
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
      {error && error.details?.events_present === false && <EmptyState variant={EmptyStateVariants.WithCliInstructionImportTrades} docker={error.details?.in_docker} />}
      {error && error.details?.events_present !== false && (
        <div role="alert" className="bg-destructive/10 text-destructive mb-6 rounded-xl border border-destructive/20 p-4 text-sm">
          Portfolio could not be loaded.
        </div>
      )}
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
              <div className="text-muted-foreground flex gap-2 items-center leading-none text-sm truncate">
                {
                  overviewData.unformatted_return > 0 && <Triangle size={16} className='stroke-success-foreground' />
                }
                {
                  overviewData.unformatted_return < 0 && <Triangle size={16} className='rotate-180 stroke-destructive-foreground' />
                }
                {
                  overviewData.unformatted_return === 0 && <TriangleDashed size={16} className='stroke-muted-foreground' />
                }
                <p>{data.returns_incomplete ? 'Known return: ' : ''}{overviewData.absolute_return}{data.returns_incomplete ? '' : ` (${overviewData.relative_return})`}</p>
              </div>
              {data.unpriced_assets > 0 && (
                <div className="text-muted-foreground mt-4 flex items-start gap-2 rounded-lg border p-3 text-sm">
                  <TriangleAlert className="mt-0.5 size-4 shrink-0" />
                  <p>
                     {data.unpriced_assets} {data.unpriced_assets === 1 ? 'asset is' : 'assets are'} unpriced. Portfolio value and return figures include known values only.
                  </p>
                </div>
              )}
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
                {data.positions.some((position) => position.value != null) && (
                  <BarList
                    data={data.positions
                      .filter((position) => position.value != null)
                    .map((position: PositionWithValueAndAllocation) => {
                      return {
                        key: position.asset_id,
                        name: `${position.share != null ? `${position.share}% · ` : ''}${position.name}`,
                        value: parseFloat(position.value!),
                        href: position.asset_class === 'Security' && position.isin
                          ? `https://duckduckgo.com/?q=${position.isin}`
                          : undefined,
                      };
                    })}
                    className="mt-4"
                    valueFormatter={formatCurrency}
                  />
                )}
                {data.positions.some((position) => position.value == null) && (
                  <div className="mt-5 space-y-2 border-t pt-4">
                    <p className="text-muted-foreground text-xs font-medium uppercase tracking-wide">Unpriced</p>
                    {data.positions
                      .filter((position) => position.value == null)
                      .map((position) => (
                        <div key={position.asset_id} className="bg-muted/40 flex items-center justify-between gap-3 rounded-lg px-3 py-2.5">
                          <div className="min-w-0">
                            <p className="truncate text-sm font-medium">{position.name}</p>
                            <p className="text-muted-foreground text-xs">{position.units} {position.unit_label}</p>
                          </div>
                          <Badge variant="outline">Unpriced</Badge>
                        </div>
                      ))}
                  </div>
                )}
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
