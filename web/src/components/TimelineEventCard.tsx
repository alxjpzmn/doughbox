import { TaxEventType, TaxRelevantEvent, TradeDirection } from '@/types/core';
import { formatCurrency, formatDate } from '@/util';
import { RiArrowLeftDownLine, RiArrowRightUpLine, RiDropLine, RiExchangeLine, RiHandCoinLine } from '@remixicon/react';
import { Card, Divider, Text } from '@tremor/react';

interface TimelineCardProps {
  timelineEvent: TaxRelevantEvent
}

const TradeCard: React.FC<TimelineCardProps> = ({ timelineEvent }) => {
  return (
    <Card>
      <h3 className="mb-4 font-semibold text-gray-900 dark:text-gray-50">{timelineEvent.direction}</h3>
      <Text>{timelineEvent.identifier}</Text>
      <Text>{timelineEvent.units} @ {formatCurrency(timelineEvent.price_unit, timelineEvent.currency)}</Text>
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
        {timelineEvent.event_type === TaxEventType.ShareInterest ? 'Share Lending Interest' : 'Cash Interest'}
      </Text>
      <Text>{timelineEvent.units} @ {formatCurrency(timelineEvent.price_unit, timelineEvent.currency)}</Text>
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
        <Text>{timelineEvent.identifier}{timelineEvent.event_type === TaxEventType.DividendAequivalent && ', Aequivalent'}</Text>
      </Text>
      <Text>{timelineEvent.units} @ {formatCurrency(timelineEvent.price_unit, timelineEvent.currency)}</Text>
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
      <Text>{timelineEvent.units} @ {formatCurrency(timelineEvent.price_unit, timelineEvent.currency)}</Text>
      <Divider />
      <div className='flex justify-between items-center'>
        <Text>{formatDate(new Date(timelineEvent?.date))}</Text>
        <RiExchangeLine className='fill-pink-400' />
      </div>
    </Card >
  )
}

export { TradeCard, InterestCard, DividendCard, FxCard };
