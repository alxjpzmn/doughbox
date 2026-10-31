import { EventType, TradeDirection, PortfolioEvent } from '@/types/core';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { ArrowDownLeftIcon, ArrowsDownUpIcon, ArrowUpRightIcon, HandCoinsIcon, ScrollIcon, ScalesIcon } from '@phosphor-icons/react';
import { Separator } from '@/components/ui/separator';
import { Skeleton } from '@/components/ui/skeleton';
import { formatCurrency, formatDate } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';

interface TimelineCardProps {
  timelineEvent: PortfolioEvent
}

const TaxStatus: React.FC<TimelineCardProps> = ({ timelineEvent }) =>
  timelineEvent.tax_supported ? null : (
    <Badge variant="outline">Tax excluded</Badge>
  )

const SkeletonCard: React.FC = () => {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <Skeleton className="h-6 w-1/4 " /> {/* Placeholder for Direction */}
        </CardTitle>
        <CardDescription>
          <Skeleton className="h-4 w-1/2" /> {/* Placeholder for Identifier */}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Skeleton className="h-6 w-3/4" /> {/* Placeholder for Units and Price */}
        <Separator className="my-2" />
        <div className="flex justify-between items-center">
          <Skeleton className="h-4 w-1/3" /> {/* Placeholder for Date */}
          <Skeleton className="h-4 w-4" /> {/* Placeholder for Direction Icon */}
        </div>
      </CardContent>
    </Card>
  )
}

const TradeCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {timelineEvent.direction}
        </CardTitle>
        <CardDescription>
          {timelineEvent.name ?? timelineEvent.identifier}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>{timelineEvent.units}{timelineEvent.unit_label ? ` ${timelineEvent.unit_label}` : ''} @ {formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.total_currency)}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center gap-2'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <div className="flex items-center gap-2">
            <TaxStatus timelineEvent={timelineEvent} />
            {timelineEvent.direction === TradeDirection.Buy ? <ArrowDownLeftIcon className='text-success-foreground' size={16} /> : <ArrowUpRightIcon className='text-red-400' size={16} />}
          </div>
        </div>
      </CardContent>
    </Card >
  )
}

const InterestCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          Interest
        </CardTitle>
        <CardDescription>
          {timelineEvent.name ?? (timelineEvent.event_type === EventType.ShareInterest ? 'Share Lending Interest' : timelineEvent.event_type === EventType.PrivateDebtInterest ? 'Private debt interest' : 'Cash Interest')}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.total_currency)}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center gap-2'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <div className="flex items-center gap-2">
            <TaxStatus timelineEvent={timelineEvent} />
            <HandCoinsIcon size={16} className='text-indigo-400' />
          </div>
        </div>
      </CardContent>
    </Card >
  )
}

const DividendCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          Dividend
        </CardTitle>
        <CardDescription>
          {timelineEvent.identifier}{timelineEvent.event_type === EventType.DividendAequivalent && ', Aequivalent'}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p>
        </p>
        <p className='text-xl font-bold'>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.total_currency)}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center gap-2'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <div className="flex items-center gap-2">
            <TaxStatus timelineEvent={timelineEvent} />
            <ScrollIcon size={16} className='text-indigo-400' />
          </div>
        </div>
      </CardContent>
    </Card >
  )
}

const FxCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          Foreign Exchange
        </CardTitle>
        <CardDescription>
          {timelineEvent.identifier}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.identifier?.slice(0, 3))} @ {formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.identifier?.slice(3, 6))} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.identifier?.slice(3, 6))}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center gap-2'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <div className="flex items-center gap-2">
            <TaxStatus timelineEvent={timelineEvent} />
            <ArrowsDownUpIcon size={16} className='text-pink-400' />
          </div>
        </div>
      </CardContent>
    </Card >
  )
}

const activityLabels: Partial<Record<EventType, string>> = {
  [EventType.Deposit]: "Deposit",
  [EventType.Withdrawal]: "Withdrawal",
  [EventType.PrincipalAdvance]: "Principal advance",
  [EventType.PrincipalRepayment]: "Principal repayment",
  [EventType.OpeningBalance]: "Opening balance",
  [EventType.BalanceReconciliation]: "Balance reconciliation",
  [EventType.Valuation]: "Manual valuation",
}

const AssetActivityCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  const isIncoming = timelineEvent.event_type === EventType.Deposit
    || timelineEvent.event_type === EventType.PrincipalAdvance
  const isSnapshot = timelineEvent.event_type === EventType.OpeningBalance
    || timelineEvent.event_type === EventType.BalanceReconciliation
  const isValuation = timelineEvent.event_type === EventType.Valuation
  const ActivityIcon = isSnapshot ? ScalesIcon : isIncoming ? ArrowDownLeftIcon : ArrowUpRightIcon

  return (
    <Card>
      <CardHeader>
        <CardTitle>{activityLabels[timelineEvent.event_type]}</CardTitle>
        <CardDescription>{timelineEvent.name ?? "Custom asset"}</CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>
          {isValuation
            ? `${formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.currency)}${timelineEvent.unit_label ? ` per ${timelineEvent.unit_label}` : ''} → ${formatCurrency(parseFloat(timelineEvent.total), timelineEvent.total_currency)}`
            : `${formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → ${formatCurrency(parseFloat(timelineEvent.total), timelineEvent.total_currency)}`}
        </p>
        {isSnapshot && <p className="text-muted-foreground mt-1 text-xs">Absolute balance · opening baseline or unexplained balance adjustment, not investment income</p>}
        <Separator className='my-2' />
        <div className='flex justify-between items-center gap-2'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent.date))}</p>
          <div className="flex items-center gap-2">
            <TaxStatus timelineEvent={timelineEvent} />
            <ActivityIcon className={isIncoming ? "text-success-foreground" : isSnapshot || isValuation ? "text-muted-foreground" : "text-destructive-foreground"} size={16} />
          </div>
        </div>
      </CardContent>
    </Card>
  )
}

export { SkeletonCard, TradeCard, InterestCard, DividendCard, FxCard, AssetActivityCard };
