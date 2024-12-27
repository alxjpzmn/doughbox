import { EventType, TradeDirection, PortfolioEvent } from '@/types/core';
import { formatCurrency, formatDate } from '@/util';
import { RiArrowLeftDownLine, RiArrowRightUpLine, RiDropLine, RiExchangeLine, RiHandCoinLine } from '@remixicon/react';
import { Card, Divider, Text } from '@tremor/react';

interface TimelineCardProps {
  timelineEvent: PortfolioEvent
}

const TradeCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <h3 className="mb-4 font-semibold text-gray-900 dark:text-gray-50">{timelineEvent.direction}</h3>
      <Text>{timelineEvent.identifier}</Text>
      <Text>{timelineEvent.units} @ {formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.currency)}</Text>
      <Divider />
      <div className='flex justify-between items-center'>
        <Text>{formatDate(new Date(timelineEvent?.date))}</Text>
        {timelineEvent.direction === TradeDirection.Buy ? <RiArrowLeftDownLine className='fill-green-400' /> : <RiArrowRightUpLine className='fill-red-400' />}
      </div>
    </Card >
  )
}

const InterestCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <h3 className="mb-4 font-semibold text-gray-900 dark:text-gray-50">Interest</h3>
      <Text>
        {timelineEvent.event_type === EventType.ShareInterest ? 'Share Lending Interest' : 'Cash Interest'}
      </Text>
      <Text>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), "EUR")}</Text>
      <Divider />
      <div className='flex justify-between items-center'>
        <Text>{formatDate(new Date(timelineEvent?.date))}</Text>
        <RiHandCoinLine className='fill-teal-400' />
      </div>
    </Card >
  )
}

const DividendCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <h3 className="mb-4 font-semibold text-gray-900 dark:text-gray-50">Dividend</h3>
      <Text>
        <Text>{timelineEvent.identifier}{timelineEvent.event_type === EventType.DividendAequivalent && ', Aequivalent'}</Text>
      </Text>
      <Text>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.currency)} → {formatCurrency(parseFloat(timelineEvent.total), "EUR")}</Text>
      <Divider />
      <div className='flex justify-between items-center'>
        <Text>{formatDate(new Date(timelineEvent?.date))}</Text>
        <RiDropLine className='fill-blue-400' />
      </div>
    </Card >
  )
}

const FxCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <h3 className="mb-4 font-semibold text-gray-900 dark:text-gray-50">Foreign Exchange</h3>
      <Text>
        <Text>{timelineEvent.identifier}</Text>
      </Text>
      <Text>{formatCurrency(parseFloat(timelineEvent.units), timelineEvent.identifier?.slice(0, 3))} @ {formatCurrency(parseFloat(timelineEvent.price_unit), timelineEvent.identifier?.slice(3, 6))} → {formatCurrency(parseFloat(timelineEvent.total), timelineEvent.identifier?.slice(3, 6))}</Text>
      <Divider />
      <div className='flex justify-between items-center'>
        <Text>{formatDate(new Date(timelineEvent?.date))}</Text>
        <RiExchangeLine className='fill-pink-400' />
      </div>
    </Card >
  )
}

export { TradeCard, InterestCard, DividendCard, FxCard };
