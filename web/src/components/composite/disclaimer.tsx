import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

export const Disclaimer = () => {
  return (
    <Card className='mt-6'>
      <CardHeader>
        <CardTitle>Disclaimer</CardTitle>
      </CardHeader>
      <CardContent>
        <p className='text-sm text-muted-foreground'>
          Doughbox is <a href="https://github.com/alxjpzmn/doughbox" target="_blank" className="underline">open source software</a> under the <a href="https://github.com/alxjpzmn/doughbox/blob/main/LICENSE" className="underline" target="_blank">MIT license</a>. It comes without any warranty or responsibility taken for the accuracy or completeness of the data shown.
        </p>
      </CardContent>
    </Card>
  )
}
