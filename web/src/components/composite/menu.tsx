import useAuth from "@/hooks/useAuth";
import { Link, useLocation } from "wouter";
import { ChartArea, ChartBar, FileChartPie, FileClock, NotepadText } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  NavigationMenu,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuList,
} from "@/components/ui/navigation-menu"
import { Button } from "@/components/ui/button";


const menuItems = [
  {
    text: 'Portfolio',
    route: '/portfolio',
    iconOutline: (props: any) => <ChartBar {...props} />,
    iconFill: (props: any) => <ChartBar {...props} />
  },
  {
    text: 'Performance',
    route: '/performance',
    iconOutline: (props: any) => <ChartArea {...props} />,
    iconFill: (props: any) => <ChartArea {...props} />
  },
  {
    text: 'Timeline',
    route: '/timeline',
    iconOutline: (props: any) => <FileClock {...props} />,
    iconFill: (props: any) => <FileClock {...props} />
  },
  {
    text: 'Positions',
    route: '/positions',
    iconOutline: (props: any) => <FileChartPie {...props} />,
    iconFill: (props: any) => <FileChartPie {...props} />
  },
  {
    text: 'Taxation',
    route: '/taxation',
    iconOutline: (props: any) => <NotepadText {...props} />,
    iconFill: (props: any) => <NotepadText {...props} />
  }
];

const Menu = () => {
  const { logout } = useAuth();
  const [location] = useLocation();

  return <NavigationMenu className="min-w-2xl my-4 flex justify-between items-center">
    <NavigationMenuList>
      {menuItems.map(menuItem =>
        <NavigationMenuItem key={menuItem.route}>
          <Link href={menuItem.route} >
            <NavigationMenuLink className={cn(menuItem.route === location ? "bg-muted" : "bg-transparent")}>
              {menuItem.text}
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
      )}
    </NavigationMenuList>
    <Button variant='outline' onClick={async () => {
      await logout()
    }} type='button'>Logout</Button>
  </NavigationMenu>
}

const MobileMenu = () => {
  const [location] = useLocation();

  return (
    <div className="w-full fixed z-10 left-0 bottom-0 h-24 bg-primary-foreground border-t border-primary-foreground-muted pt-6 pb-12 flex justify-around items-center">
      {menuItems.map((menuItem) => (
        <Link
          href={menuItem.route}
          key={menuItem.route}
          className="flex flex-col items-center gap-1 flex-1"
        >
          {location !== menuItem.route
            ? menuItem.iconOutline({ className: 'stroke-muted-foreground', size: 24 })
            : menuItem.iconFill({ className: 'stroke-foreground', size: 24 })}
          <p
            className={cn(
              'text-xs max-[389px]:hidden min-[390px]:block', // Hide text on iPhone SE-sized screens
              location !== menuItem.route ? 'text-muted-foreground' : 'text-foreground'
            )}
          >
            {menuItem.text}
          </p>
        </Link>
      ))}
    </div>
  )
}

export { Menu, MobileMenu };
