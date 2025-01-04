import { Color, DeltaType } from '@tremor/react';
import clsx, { type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"
import { Cache } from 'swr';
import { formatRelative, fromUnixTime } from 'date-fns';

export const BASE_URL = '/api';

interface MutateRequestOptions {
  method: "POST" | "PUT" | "PATCH" | "DELETE";
}

const defaultHeaders = {
};

type GenericPayload = { [key: string]: any };

type FetcherArgs = [string, RequestInit?];


const handleResponseError = async (res: Response) => {
  let errorData;
  const contentType = res.headers.get('content-type') || '';

  try {
    if (contentType.includes('application/json')) {
      errorData = await res.json();
    } else {
      errorData = {
        status: res.status,
        error: 'UnknownError',
        message: res.statusText || 'An unexpected error occurred.',
        details: await res.text(),
      };
    }
  } catch (err) {
    errorData = {
      status: res.status,
      error: 'UnknownError',
      message: 'An unexpected error occurred.',
      details: 'No further details available',
    };
  }

  if (errorData && errorData.status && errorData.error && errorData.message) {
    throw errorData;
  } else {
    throw {
      status: res.status,
      error: 'UnknownError',
      message: 'An unexpected error occurred.',
      details: errorData.details || 'No further details available',
    };
  }
};

export const fetcher = async (...args: FetcherArgs) => {
  try {
    const res = await fetch(...args);

    if (!res.ok) {
      await handleResponseError(res);
    }

    const contentType = res.headers.get('content-type');
    if (contentType && contentType.includes('application/json')) {
      return await res.json();
    } else {
      return await res.text();
    }
  } catch (error: any) {
    console.error('Error caught in fetcher:', error);
    return Promise.reject({
      status: error.status || 500,
      error: error.error || 'NetworkError',
      message: error.message || 'A network error occurred.',
      details: error.details || error.message || 'No further details available',
    });
  }
};

export const sendMutateRequest = async (
  apiPath: string,
  payload?: GenericPayload,
  requestOptions: MutateRequestOptions = { method: "POST" },
) => {
  try {
    const res = await fetch(`${apiPath}`, {
      method: requestOptions.method,
      headers: { "Content-Type": "application/json", ...defaultHeaders },
      body: JSON.stringify(payload),
    });

    if (!res.ok) {
      await handleResponseError(res);
    }

    return res;
  } catch (error: any) {
    console.error({ message: error.message, type: 'error' });
    return Promise.reject({
      status: error.status || 500,
      error: error.error || 'UnknownError',
      message: error.message || 'An unknown error occurred.',
      details: error.details || 'No further details available',
    });
  }
};

export const clearSWRCache = (cache: Cache<any>) => {
  for (let cache_key of cache.keys()) {
    cache.delete(cache_key);
  }
};

export const formatCurrency = (number: number, currency: string = 'EUR') =>
  `${new Intl.NumberFormat(getBrowserLocale(), {
    style: 'currency',
    currency,
  }).format(number)}`;

export const formatRelativeAmount = (number: number) =>
  `${Intl.NumberFormat('us').format(number).toString()}%`;

export const colors: { [key: string]: Color } = {
  increase: 'emerald',
  moderateIncrease: 'emerald',
  unchanged: 'orange',
  moderateDecrease: 'rose',
  decrease: 'rose',
};

export const getBrowserLocale = () => {
  if (navigator.languages != undefined)
    return navigator.languages[0];
  return navigator.language;
}

export const getDeltaType = (change: number): DeltaType => {
  if (change > 5) {
    return 'increase';
  } else if (change > 1) {
    return 'moderateIncrease';
  } else if (change > -1) {
    return 'unchanged';
  } else if (change > -5) {
    return 'moderateDecrease';
  } else {
    return 'decrease';
  }
};

export const formatUnixTimestampRelative = (input: number): string => {
  if (input) {
    return formatRelative(fromUnixTime(input), new Date());
  }
  return '';
};

export const formatDate = (date: Date): string => {
  return new Intl.DateTimeFormat(getBrowserLocale()).format(date)
}

// Tremor cx [v0.0.0]
export function cx(...args: ClassValue[]) {
  return twMerge(clsx(...args))
}
// Tremor focusInput [v0.0.1]
export const focusInput = [
  // base
  "focus:ring-2",
  // ring color
  "focus:ring-blue-200 focus:dark:ring-blue-700/30",
  // border color
  "focus:border-blue-500 focus:dark:border-blue-700",
]

// Tremor hasErrorInput [v0.0.1]

export const hasErrorInput = [
  // base
  "ring-2",
  // border color
  "border-red-500 dark:border-red-700",
  // ring color
  "ring-red-200 dark:ring-red-700/30",
]

// Tremor focusRing [v0.0.1]

export const focusRing = [
  // base
  "outline outline-offset-2 outline-0 focus-visible:outline-2",
  // outline color
  "outline-blue-500 dark:outline-blue-500",
]
