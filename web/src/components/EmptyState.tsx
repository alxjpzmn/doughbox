import React from 'react'
import { Card, Text } from '@tremor/react'
import { Badge } from './Badge'

export enum EmptyStateVariants {
  Default,
  WithCliInstructionImport,
  WithCliInstructionImportTrades,
  WithCliInstructionPerformance,
  WithCliInstructionTaxation,

}

interface EmptyStateProps { variant?: EmptyStateVariants, docker?: boolean }

const EmptyState: React.FC<EmptyStateProps> = ({ variant = EmptyStateVariants.Default, docker = false }) => {

  return (
    <Card>
      {variant === EmptyStateVariants.Default && <>
        <Text>
          No events found. Try changing your filter or import events.
        </Text>
      </>}
      {variant === EmptyStateVariants.WithCliInstructionImport && <>
        <Text>
          You haven't imported any events (e.g. trades, dividends) yet.
          Please run:
        </Text>
        <Badge variant='neutral' className='mt-4'>
          <Text className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox import folder-with-your-brokerage-statements`}</Text>
        </Badge>
      </>}
      {variant === EmptyStateVariants.WithCliInstructionImportTrades && <>
        <Text>
          You haven't imported any trades yet.
          Please run:
        </Text>
        <Badge variant='neutral' className='mt-4'>
          <Text className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox import folder-with-your-brokerage-statements`}</Text>
        </Badge>
      </>}
      {variant === EmptyStateVariants.WithCliInstructionPerformance && <>
        <Text>
          You haven't run a performance calculation yet. Please run:
        </Text>
        <Badge variant='neutral' className='mt-4'>
          <Text className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox performance`}</Text>
        </Badge>
      </>}
      {variant === EmptyStateVariants.WithCliInstructionTaxation && <>
        <Text>
          You haven't run a taxation calculation yet. Please run:
        </Text>
        <Badge variant='neutral' className='mt-4'>
          <Text className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox taxation`}</Text>
        </Badge>
      </>}
    </Card>
  )

}

export default EmptyState
