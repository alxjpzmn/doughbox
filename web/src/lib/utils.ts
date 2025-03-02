import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"
import { formatRelative, fromUnixTime } from 'date-fns';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export const formatCurrency = (number: number, currency: string = 'EUR') =>
  `${new Intl.NumberFormat(getBrowserLocale(), {
    style: 'currency',
    currency,
  }).format(number)}`;

export const formatRelativeAmount = (number: number) =>
  `${Intl.NumberFormat('us').format(number).toString()}%`;

export const getBrowserLocale = () => {
  if (navigator.languages != undefined)
    return navigator.languages[0];
  return navigator.language;
}

export const formatUnixTimestampRelative = (input: number): string => {
  if (input) {
    return formatRelative(fromUnixTime(input), new Date());
  }
  return '';
};

export const formatDate = (date: Date): string => {
  return new Intl.DateTimeFormat(getBrowserLocale()).format(date)
}

export const colorMetric = (metric: string) => {
  return parseFloat(metric) === 0
    ? "text-muted-foreground"
    : parseFloat(metric) < 0
      ? "text-destructive-foreground"
      : "text-success-foreground"

}
