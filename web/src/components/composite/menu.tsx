import { useEffect, useRef, useState } from 'react';
import useAuth from '@/hooks/useAuth';
import { Link, useLocation } from 'wouter';
import { ChartBarIcon, TrayArrowDownIcon, NotepadIcon, GearSixIcon, ArrowsLeftRightIcon } from '@phosphor-icons/react';
import { Role } from '@/types/core';
import { cn } from '@/lib/utils';
import { NavigationMenu, NavigationMenuItem, NavigationMenuLink, NavigationMenuList } from '@/components/ui/navigation-menu';
import { Button } from '@/components/ui/button';

const itemActive = (route: string, location: string) =>
  route === '/overview' ? location === '/overview' || location === '/performance' : route === location;
const menuItems = [
  { text: 'Portfolio', route: '/overview', icon: ChartBarIcon },
  { text: 'Data', route: '/instruments', icon: TrayArrowDownIcon },
  { text: 'Taxation', route: '/taxation', icon: NotepadIcon },
  { text: 'Trading', route: '/trading', icon: ArrowsLeftRightIcon },
  { text: 'Settings', route: '/settings', icon: GearSixIcon },
];

const AccountActions = () => {
  const { auth, logout } = useAuth();
  const [location] = useLocation();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const submitting = useRef(false);
  return <div className="flex w-full min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-sm">
    <span className="min-w-0 flex-1 break-words" aria-label="Current account"><span className="font-medium">{auth?.user?.username}</span> <span className="text-muted-foreground">· {auth?.user?.role}</span></span>
    <Link href="/security" className="inline-flex min-h-11 items-center rounded-md px-1 underline-offset-4 hover:underline" aria-current={location === '/security' ? 'page' : undefined}>Security</Link>
    {auth?.user?.role === Role.Admin && <Link href="/users" className="inline-flex min-h-11 items-center rounded-md px-1 underline-offset-4 hover:underline" aria-current={location === '/users' ? 'page' : undefined}>Users &amp; instance</Link>}
    <Button variant="outline" disabled={busy} type="button" className="min-h-11" onClick={async () => {
      if (submitting.current) return;
      submitting.current = true;
      setBusy(true);
      setFailure(null);
      try { await logout(); }
      catch (error) {
        if (!(error instanceof Error && error.name === 'AbortError')) setFailure(error && typeof error === 'object' && 'message' in error && typeof error.message === 'string'
          ? error.message : 'Logout could not be completed. Please retry.');
      } finally { submitting.current = false; setBusy(false); }
    }}>{busy ? 'Logging out…' : 'Logout'}</Button>
    {failure && <p role="alert" className="w-full break-words text-xs">{failure}</p>}
  </div>;
};

const Menu = () => {
  const [location] = useLocation();
  return <div className="my-4 space-y-2">
    <AccountActions />
    <NavigationMenu aria-label="Dashboard" className="w-full max-w-none justify-start">
      <NavigationMenuList className="flex-wrap justify-start">
        {menuItems.map(menuItem => <NavigationMenuItem key={menuItem.route}>
          <NavigationMenuLink asChild active={itemActive(menuItem.route, location)} className={cn('min-h-11 flex-row items-center justify-center gap-1.5 px-2', itemActive(menuItem.route, location) ? 'bg-muted' : 'bg-transparent')}>
            <Link href={menuItem.route} aria-current={itemActive(menuItem.route, location) ? 'page' : undefined}>{menuItem.text}</Link>
          </NavigationMenuLink>
        </NavigationMenuItem>)}
      </NavigationMenuList>
    </NavigationMenu>
  </div>;
};

const MobileMenu = () => {
  const [location] = useLocation();
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    const node = nav.current;
    if (!node) return;
    const sync = () => document.documentElement.style.setProperty('--mobile-nav-height', `${node.getBoundingClientRect().height}px`);
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return <nav ref={nav} aria-label="Dashboard" className="z-10 w-full shrink-0 border-t bg-background px-2 pt-1 pb-[calc(1rem+env(safe-area-inset-bottom))]">
    <AccountActions />
    <div className="grid grid-cols-5 gap-0.5">
      {menuItems.map(menuItem => <Link href={menuItem.route} key={menuItem.route} className="flex min-h-11 min-w-0 flex-col items-center justify-center gap-1 rounded-md focus-visible:outline focus-visible:outline-2" aria-label={menuItem.text} aria-current={itemActive(menuItem.route, location) ? 'page' : undefined}>
        <menuItem.icon className={itemActive(menuItem.route, location) ? 'text-foreground' : 'text-muted-foreground'} size={20} aria-hidden="true" />
        <span className={cn('w-full truncate text-center text-[10px] min-[400px]:text-[11px]', itemActive(menuItem.route, location) ? 'text-foreground' : 'text-muted-foreground')}>{menuItem.text}</span>
      </Link>)}
    </div>
  </nav>;
};

export { Menu, MobileMenu };
