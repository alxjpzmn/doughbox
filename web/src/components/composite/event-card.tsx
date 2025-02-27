import { EventType, TradeDirection, PortfolioEvent } from '@/types/core';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { ArrowDownLeft, ArrowDownUp, ArrowUpRight, HandCoins, Scroll } from 'lucide-react';
import { Separator } from '@/components/ui/separator';
import { Skeleton } from '@/components/ui/skeleton';
import { formatCurrency, formatDate } from '@/lib/utils';

interface TimelineCardProps {
  timelineEvent: PortfolioEvent
}

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
          {timelineEvent.identifier}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>{timelineEvent.units} @ {formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.currency)}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          {timelineEvent.direction === TradeDirection.Buy ? <ArrowDownLeft className='stroke-success-foreground' size={16} /> : <ArrowUpRight className='stroke-red-400' size={16} />}
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
          {timelineEvent.event_type === EventType.ShareInterest ? 'Share Lending Interest' : 'Cash Interest'}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <p className='text-xl font-bold'>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), "EUR")}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <HandCoins size={16} className='stroke-indigo-400' />
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
        <p className='text-xl font-bold'>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), "EUR")}</p>
        <Separator className='my-2' />
        <div className='flex justify-between items-center'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <Scroll size={16} className='stroke-indigo-400' />
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
        <div className='flex justify-between items-center'>
          <p className='text-muted-foreground text-sm'>{formatDate(new Date(timelineEvent?.date))}</p>
          <ArrowDownUp size={16} className='stroke-pink-400' />
        </div>
      </CardContent>
    </Card >
  )
}

export { SkeletonCard, TradeCard, InterestCard, DividendCard, FxCard };
