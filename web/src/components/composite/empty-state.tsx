import { Card, CardContent } from '@/components/ui/card'

export enum EmptyStateVariants {
  Default,
  WithCliInstructionImport,
  WithCliInstructionImportTrades,
  WithCliInstructionPerformance,
  WithCliInstructionTaxation,
}

interface EmptyStateProps { variant?: EmptyStateVariants, docker?: boolean }

const command = (docker: boolean, rest: string) =>
  `${docker ? 'docker container exec -it container_name ' : ''}./doughbox ${rest}`

const EmptyState = ({ variant = EmptyStateVariants.Default, docker = false }: EmptyStateProps) => {
  const copy = {
    [EmptyStateVariants.Default]: {
      text: 'No events found. Try changing your filter or import events.',
      cli: null,
    },
    [EmptyStateVariants.WithCliInstructionImport]: {
      text: "You haven't imported any events (e.g. trades, dividends) yet. Please run:",
      cli: command(docker, 'import folder-with-your-brokerage-statements'),
    },
    [EmptyStateVariants.WithCliInstructionImportTrades]: {
      text: "You haven't imported any trades yet. Please run:",
      cli: command(docker, 'import folder-with-your-brokerage-statements'),
    },
    [EmptyStateVariants.WithCliInstructionPerformance]: {
      text: "You haven't run a performance calculation yet. Please run:",
      cli: command(docker, 'performance'),
    },
    [EmptyStateVariants.WithCliInstructionTaxation]: {
      text: "You haven't run a taxation calculation yet. Please run:",
      cli: command(docker, 'taxation'),
    },
  }[variant]

  return (
    <Card>
      <CardContent className="flex flex-col gap-3 p-6">
        <p>{copy.text}</p>
        {copy.cli && (
          <code className="w-fit max-w-full rounded-md bg-primary px-2.5 py-1 font-mono text-xs text-primary-foreground break-all">
            {copy.cli}
          </code>
        )}
      </CardContent>
    </Card>
  )
}

export default EmptyState
