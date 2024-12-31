import { Drawer, DrawerTrigger, DrawerContent, DrawerHeader, DrawerTitle, DrawerBody, DrawerFooter, DrawerClose } from "@/components/Drawer";
import { Button } from "@/components/Button";
import useAuth from "@/hooks/useAuth";
import { Text } from "@tremor/react";
import { Link, useLocation } from "wouter";
import { TabNavigation, TabNavigationLink } from "@/components/TabNavigation";

const menuItems = [
  { text: 'Portfolio', route: '/portfolio' },
  { text: 'Performance', route: '/performance' },
  { text: 'Timeline', route: '/timeline' },
  { text: 'Positions', route: '/positions' },
  { text: 'Taxation', route: '/taxation' }
]

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
  const { logout } = useAuth();
  const [location] = useLocation();

  return (
    <div className="w-full mb-4 flex justify-end">
      <Drawer>
        <DrawerTrigger asChild>
          <Button variant="secondary">Menu</Button>
        </DrawerTrigger>
        <DrawerContent className="sm:max-w-lg">
          <DrawerHeader>
            <DrawerTitle>Menu</DrawerTitle>
          </DrawerHeader>
          <DrawerBody className="flex flex-col">
            <Text className="grid grid-cols-1 gap-6 text-lg font-bold">
              {menuItems.map(menuItem =>
                <span className="flex items-center" key={menuItem.route}>{location === `${menuItem.route}` && <span className="w-2 h-2 mr-2 bg-green-400 rounded-full" />}<Link href={menuItem.route}><DrawerClose>{menuItem.text}</DrawerClose></Link></span>
              )}
            </Text>
          </DrawerBody>
          <DrawerFooter className="mt-6">
            <DrawerClose className="text-red-400 text-lg pb-2 font-bold" onClick={async () => {
              await logout()
            }} type='button'>Logout</DrawerClose>
          </DrawerFooter>
        </DrawerContent>
      </Drawer>
    </div>
  )
}

export { Menu, MobileMenu };
