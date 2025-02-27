import React from 'react'
import { Card, CardContent } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'

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
      <CardContent>
        {variant === EmptyStateVariants.Default && <>
          <p>
            No events found. Try changing your filter or import events.
          </p>
        </>}
        {variant === EmptyStateVariants.WithCliInstructionImport && <>
          <p>
            You haven't imported any events (e.g. trades, dividends) yet.
            Please run:
          </p>
          <Badge variant='default' className='mt-4'>
            <p className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox import folder-with-your-brokerage-statements`}</p>
          </Badge>
        </>}
        {variant === EmptyStateVariants.WithCliInstructionImportTrades && <>
          <p>
            You haven't imported any trades yet.
            Please run:
          </p>
          <Badge variant='default' className='mt-4'>
            <p className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox import folder-with-your-brokerage-statements`}</p>
          </Badge>
        </>}
        {variant === EmptyStateVariants.WithCliInstructionPerformance && <>
          <p>
            You haven't run a performance calculation yet. Please run:
          </p>
          <Badge variant='default' className='mt-4'>
            <p className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox performance`}</p>
          </Badge>
        </>}
        {variant === EmptyStateVariants.WithCliInstructionTaxation && <>
          <p>
            You haven't run a taxation calculation yet. Please run:
          </p>
          <Badge variant='default' className='mt-4'>
            <p className='font-mono'>{`${docker ? 'docker container exec -it container_name' : ''} ./doughbox taxation`}</p>
          </Badge>
        </>}
      </CardContent>
    </Card>
  )

}

export default EmptyState
