import { Callout } from './Callout'

export const Disclaimer = () => {
  return (

    <Callout variant="default" title="Disclaimer" className="mt-6">
      Doughbox is <a href="https://github.com/alxjpzmn/doughbox" target="_blank" className="underline">open source software</a> under the <a href="https://github.com/alxjpzmn/doughbox/blob/main/LICENSE" className="underline" target="_blank">MIT license</a>. It comes without any warranty or responsibility taken for the accuracy or completeness of the data shown.
    </Callout>
  )
}
