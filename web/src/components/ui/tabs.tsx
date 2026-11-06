import * as React from "react"
import * as TabsPrimitive from "@radix-ui/react-tabs"

import { cn } from "@/lib/utils"

function Tabs({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function TabsList({
  className,
  dock = false,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List> & { dock?: boolean | "stacked" }) {
  const bar = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    if (dock !== true) return;
    const node = bar.current;
    if (!node) return;
    const sync = () => document.documentElement.style.setProperty("--section-tabs-height", `${node.getBoundingClientRect().height}px`);
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(node);
    return () => observer.disconnect();
  }, [dock]);
  return (
    <TabsPrimitive.List
      ref={bar}
      data-slot="tabs-list"
      className={cn(
        "bg-muted text-muted-foreground inline-flex h-9 w-fit items-center justify-center rounded-lg p-[3px]",
        dock && "max-md:fixed max-md:inset-x-0 max-md:z-20 max-md:mb-0 max-md:flex max-md:h-auto max-md:w-full max-md:flex-nowrap max-md:items-stretch max-md:justify-start max-md:gap-1 max-md:overflow-x-auto max-md:rounded-none max-md:border-x-0 max-md:border-t max-md:bg-background max-md:p-2 max-md:shadow-none max-md:bottom-(--mobile-nav-height) max-md:[&>[data-slot=tabs-trigger]]:h-11 max-md:[&>[data-slot=tabs-trigger]]:min-h-11 max-md:[&>[data-slot=tabs-trigger]]:flex-none max-md:[&>[data-slot=tabs-trigger]]:shrink-0 max-md:[&>[data-slot=tabs-trigger]]:rounded-md max-md:[&>[data-slot=tabs-trigger]]:border max-md:[&>[data-slot=tabs-trigger]]:border-input max-md:[&>[data-slot=tabs-trigger]]:bg-background max-md:[&>[data-slot=tabs-trigger]]:px-3 max-md:[&>[data-slot=tabs-trigger]]:text-sm max-md:[&>[data-slot=tabs-trigger]]:font-medium max-md:[&>[data-slot=tabs-trigger]]:text-muted-foreground max-md:[&>[data-slot=tabs-trigger]]:shadow-sm max-md:[&>[data-slot=tabs-trigger][data-state=active]]:border-transparent max-md:[&>[data-slot=tabs-trigger][data-state=active]]:bg-primary max-md:[&>[data-slot=tabs-trigger][data-state=active]]:text-primary-foreground max-md:[&>[data-slot=tabs-trigger][data-state=active]]:shadow-sm",
        dock === "stacked" && "max-md:bottom-[calc(var(--mobile-nav-height,0px)+var(--section-tabs-height,0px))]",
        className
      )}
      {...props}
    />
  )
}

function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "data-[state=active]:bg-background data-[state=active]:text-foreground focus-visible:border-ring focus-visible:ring-ring/50 inline-flex h-[calc(100%-1px)] flex-1 cursor-pointer items-center justify-center gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium whitespace-nowrap transition-[color,box-shadow] focus-visible:ring-[3px] focus-visible:outline-1 disabled:pointer-events-none disabled:opacity-50 data-[state=active]:shadow-sm",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn("flex-1 outline-none", className)}
      {...props}
    />
  )
}

export { Tabs, TabsList, TabsTrigger, TabsContent }
