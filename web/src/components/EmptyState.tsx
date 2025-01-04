import React from 'react'
import { Card, Text } from '@tremor/react'
import { Badge } from './Badge'

interface EmptyStateProps { }

const EmptyState: React.FC<EmptyStateProps> = ({ }) => {


  return (
    <Card>
      <Text>
        It seems you don't have any events (e.g. trades, dividends) imported yet.
        To get started, please run the following command:
      </Text>
      <Badge variant='neutral' className='mt-4'>
        <Text className='font-mono'>./doughbox import folder-with-your-brokerage-statements</Text>
      </Badge>
    </Card>
  )

}

export default EmptyState
