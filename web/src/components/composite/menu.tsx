import { useEffect, useRef } from "react";
import useAuth from "@/hooks/useAuth";
import { Link, useLocation } from "wouter";
import { ChartBarIcon, TrayArrowDownIcon, NotepadIcon, GearSixIcon, ArrowsLeftRightIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import {
  NavigationMenu,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuList,
} from "@/components/ui/navigation-menu"
import { Button } from "@/components/ui/button";

const itemActive = (route: string, location: string) =>
  route === '/overview' ? location === '/overview' || location === '/performance' : route === location;
const menuItems = [
  { text: 'Portfolio', route: '/overview', icon: ChartBarIcon },
  { text: 'Data', route: '/instruments', icon: TrayArrowDownIcon },
  { text: 'Taxation', route: '/taxation', icon: NotepadIcon },
  { text: 'Trading', route: '/trading', icon: ArrowsLeftRightIcon },
  { text: 'Settings', route: '/settings', icon: GearSixIcon },
];

const Menu = () => {
  const { logout } = useAuth();
  const [location] = useLocation();

  return <NavigationMenu aria-label="Dashboard" className="my-4 w-full max-w-none flex-wrap justify-between gap-2">
    <NavigationMenuList className="flex-wrap justify-start">
      {menuItems.map(menuItem =>
        <NavigationMenuItem key={menuItem.route}>
          <NavigationMenuLink asChild active={itemActive(menuItem.route, location)} className={cn("min-h-11 flex-row items-center justify-center gap-1.5 px-2", itemActive(menuItem.route, location) ? "bg-muted" : "bg-transparent")}>
            <Link href={menuItem.route} aria-current={itemActive(menuItem.route, location) ? "page" : undefined}>
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
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    const node = nav.current;
    if (!node) return;
    const sync = () => document.documentElement.style.setProperty("--mobile-nav-height", `${node.getBoundingClientRect().height}px`);
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <nav ref={nav} aria-label="Dashboard" className="z-10 grid w-full shrink-0 grid-cols-5 gap-0.5 border-t bg-background px-1 pt-2 pb-[calc(1rem+env(safe-area-inset-bottom))]">
      {menuItems.map((menuItem) => (
        <Link
          href={menuItem.route}
          key={menuItem.route}
          className="flex min-h-11 min-w-0 flex-col items-center justify-center gap-1 rounded-md focus-visible:outline focus-visible:outline-2"
          aria-label={menuItem.text}
          aria-current={itemActive(menuItem.route, location) ? 'page' : undefined}
        >
          <menuItem.icon
            className={itemActive(menuItem.route, location) ? 'text-foreground' : 'text-muted-foreground'}
            size={20}
            aria-hidden="true"
          />
          <p
            className={cn(
              'w-full truncate text-center text-[10px] min-[400px]:text-[11px]',
              itemActive(menuItem.route, location) ? 'text-foreground' : 'text-muted-foreground'
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
