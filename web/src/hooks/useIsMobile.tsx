import { useWindowSize } from "react-use";

const useIsMobile = (): boolean => {
  const { width } = useWindowSize();
  // corresponds to Tailwind's 'md':
  // https://tailwindcss.com/docs/responsive-design
  return width < 768;
}

export default useIsMobile;
