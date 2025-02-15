import useAuth from "@/hooks/useAuth";
import { Link, useLocation } from "wouter";
import { TabNavigation, TabNavigationLink } from "@/components/TabNavigation";
import { RiFileListFill, RiFileListLine, RiLineChartFill, RiLineChartLine, RiPieChartFill, RiPieChartLine, RiTableFill, RiTableLine, RiTimeFill, RiTimeLine } from "@remixicon/react";
import { cx } from "@/util";

const menuItems = [
  {
    text: 'Portfolio',
    route: '/portfolio',
    iconOutline: (props: any) => <RiPieChartLine {...props} />,
    iconFill: (props: any) => <RiPieChartFill {...props} />
  },
  {
    text: 'Performance',
    route: '/performance',
    iconOutline: (props: any) => <RiLineChartLine {...props} />,
    iconFill: (props: any) => <RiLineChartFill {...props} />
  },
  {
    text: 'Timeline',
    route: '/timeline',
    iconOutline: (props: any) => <RiTimeLine {...props} />,
    iconFill: (props: any) => <RiTimeFill {...props} />
  },
  {
    text: 'Positions',
    route: '/positions',
    iconOutline: (props: any) => <RiTableLine {...props} />,
    iconFill: (props: any) => <RiTableFill {...props} />
  },
  {
    text: 'Taxation',
    route: '/taxation',
    iconOutline: (props: any) => <RiFileListLine {...props} />,
    iconFill: (props: any) => <RiFileListFill {...props} />
  }
];

const Menu = () => {
  const { logout } = useAuth();
  const [location] = useLocation();

  return <TabNavigation className="mb-10 flex items-center justify-between">
    <div className="flex">
      {menuItems.map(menuItem =>
        <TabNavigationLink asChild active={location === `${menuItem.route}`} key={menuItem.route}>
          <Link href={menuItem.route}>{menuItem.text}</Link>
        </TabNavigationLink>
      )}
    </div>
    <button className="text-red-400 text-sm pb-2" onClick={async () => {
      await logout()
    }} type='button'>Logout</button>
  </TabNavigation>
}

const MobileMenu = () => {
  const [location] = useLocation();

  return (
    <div className="w-full fixed z-10 left-0 bottom-0 h-24 bg-gray-50 dark:bg-gray-900 border-t border-gray-200 dark:border-gray-800 pt-6 pb-12 px-8 flex justify-between items-center gap-1">
      {menuItems.map(menuItem =>
        <span className="flex items-center" key={menuItem.route}>
          <Link href={menuItem.route} className='flex flex-col items-center gap-2'>
            {location !== menuItem.route ? menuItem.iconOutline({ className: 'text-gray-500', size: 24 }) : menuItem.iconFill({ className: 'text-blue-500', size: 24 })}
            <p className={cx('text-xs', location !== menuItem.route ? 'text-gray-500' : 'text-blue-500')}>{menuItem.text}</p>
          </Link>
        </span>
      )}
    </div>
  )
}

export { Menu, MobileMenu };
