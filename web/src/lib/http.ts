export const BASE_URL = '/api';
import { Cache } from 'swr';

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
