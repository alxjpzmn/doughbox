import useAuth from "@/hooks/useAuth";
import { Link, useLocation } from "wouter";
import { ChartLineIcon, ChartBarIcon, TrayArrowDownIcon, NotepadIcon, GearSixIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import {
  NavigationMenu,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuList,
} from "@/components/ui/navigation-menu"
import { Button } from "@/components/ui/button";


const menuItems = [
  { text: 'Portfolio', route: '/overview', icon: ChartBarIcon },
  { text: 'Performance', route: '/performance', icon: ChartLineIcon },
  { text: 'Data', route: '/instruments', icon: TrayArrowDownIcon },
  { text: 'Taxation', route: '/taxation', icon: NotepadIcon },
  { text: 'Settings', route: '/settings', icon: GearSixIcon },
];

const Menu = () => {
  const { logout } = useAuth();
  const [location] = useLocation();

  return <NavigationMenu aria-label="Dashboard" className="my-4 w-full max-w-none flex-wrap justify-between gap-2">
    <NavigationMenuList className="flex-wrap justify-start">
      {menuItems.map(menuItem =>
        <NavigationMenuItem key={menuItem.route}>
          <NavigationMenuLink asChild active={menuItem.route === location} className={cn("min-h-11 flex-row items-center justify-center gap-1.5 px-2", menuItem.route === location ? "bg-muted" : "bg-transparent")}>
            <Link href={menuItem.route} aria-current={menuItem.route === location ? "page" : undefined}>
              {menuItem.text}
            </Link>
          </NavigationMenuLink>
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
    <nav aria-label="Dashboard" className="z-10 grid w-full shrink-0 grid-cols-5 gap-1 border-t bg-background px-2 pt-2 pb-[calc(1rem+env(safe-area-inset-bottom))]">
      {menuItems.map((menuItem) => (
        <Link
          href={menuItem.route}
          key={menuItem.route}
          className="flex min-h-11 min-w-0 flex-col items-center justify-center gap-1 rounded-md focus-visible:outline focus-visible:outline-2"
          aria-label={menuItem.text}
          aria-current={location === menuItem.route ? 'page' : undefined}
        >
          <menuItem.icon
            className={location === menuItem.route ? 'text-foreground' : 'text-muted-foreground'}
            size={20}
            aria-hidden="true"
          />
          <p
            className={cn(
              'w-full truncate text-center text-[11px]',
              location !== menuItem.route ? 'text-muted-foreground' : 'text-foreground'
            )}
          >
            {menuItem.text}
          </p>
        </Link>
      ))}
    </nav>
  )
}

export { Menu, MobileMenu };
