import useSwr from 'swr';
import { PortfolioOverview, PositionWithValueAndAllocation } from '@/types/core';
import EmptyState, { EmptyStateVariants } from '@/components/composite/empty-state';
import { Skeleton } from '@/components/ui/skeleton';
import { Disclaimer } from '@/components/composite/disclaimer';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { BarList } from '@/components/composite/bar-list';
import { BASE_URL, fetcher } from '@/lib/http';
import { formatCurrency, formatRelativeAmount } from '@/lib/utils';
import { ArrowUpIcon, ArrowDownIcon, MinusIcon, WarningIcon } from '@phosphor-icons/react';
import { Badge } from '@/components/ui/badge';

const Portfolio = () => {
  const { data, isLoading, error } = useSwr<PortfolioOverview>(`${BASE_URL}/portfolio`, fetcher, { refreshInterval: 60_000 });

  let overviewData = {
    title: data && (data.security_history_gaps.length > 0 || !data.valuation_complete || data.positions.some((position) => position.value == null)) ? 'Partial Portfolio Value' : 'Current Portfolio Value',
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
              <div className="mb-2 min-w-0">
                <p className="break-words text-2xl font-bold tabular-nums min-[400px]:text-3xl sm:text-4xl">{overviewData.portfolio_value}</p>
              </div>
              {data.valuation_complete ? <div className="text-muted-foreground flex flex-wrap gap-2 items-center leading-none text-sm">
                {
                  overviewData.unformatted_return > 0 && <ArrowUpIcon size={16} className='text-success-foreground' />
                }
                {
                  overviewData.unformatted_return < 0 && <ArrowDownIcon size={16} className='text-destructive-foreground' />
                }
                {
                  overviewData.unformatted_return === 0 && <MinusIcon size={16} className='text-muted-foreground' />
                }
                <p className="min-w-0 break-words">{data.returns_incomplete ? 'Known P&L: ' : 'P&L: '}{overviewData.absolute_return}{data.returns_incomplete ? '' : ` (${overviewData.relative_return})`}</p>
              </div> : <p className="text-sm text-destructive-foreground">
                Excludes holdings without a usable price: {data.unpriced_isins.join(', ')}. Returns are unavailable until all securities are priced.
              </p>}
              <p className="mt-3 text-xs text-muted-foreground">P&amp;L includes trading gains after fees, recorded gross EUR security dividends, and recorded cash/debt interest, including unlinked interest once for the full portfolio. Recorded income: {formatCurrency(Number(data.income))}, before withholding tax; no projected income or deemed distributions. Percentage uses cumulative gross EUR purchase spend or contributions, not current cost basis.</p>
              {data.security_history_gaps.length > 0 && (
                <div role="alert" className="text-muted-foreground mt-4 space-y-2 rounded-lg border p-3 text-sm">
                  <p className="font-medium">Incomplete security acquisition history</p>
                  <p>Trading P&amp;L for the affected instruments is unavailable and excluded from Known P&amp;L; recorded income remains included. Holdings reflect recorded trades and may also be incomplete. Import or reconcile the missing acquisitions or transfers. Performance snapshots remain blocked until the history is complete.</p>
                  <ul className="space-y-2">
                    {data.security_history_gaps.map((gap) => (
                      <li key={`${gap.broker}/${gap.isin}/${gap.date}`}>
                        <span className="font-medium">{gap.isin} · {gap.broker}</span>
                        <span className="block text-muted-foreground">{new Date(gap.date).toLocaleString()}: sold {gap.sold_units} split-adjusted shares; recorded inventory {gap.available_units}.</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {data.unpriced_assets > 0 && (
                <div className="text-muted-foreground mt-4 flex items-start gap-2 rounded-lg border p-3 text-sm">
                  <WarningIcon className="mt-0.5 size-4 shrink-0" />
                  <p>
                    {data.unpriced_assets} custom {data.unpriced_assets === 1 ? 'asset has' : 'assets have'} incomplete valuation or cost data. Portfolio value and return figures include known values only.
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
                  Holdings
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
