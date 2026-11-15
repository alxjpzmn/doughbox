import { BASE_URL } from '@/lib/http';

const importedEndpoints = [
  'timeline', 'instruments', 'brokers', 'prices', 'portfolio', 'positions', 'trades',
  'buy-ins', 'dividends', 'interest', 'fx-conversions', 'performance',
  'performance_overview', 'past_performance', 'taxation', 'data-imports',
].map(endpoint => `${BASE_URL}/${endpoint}`);

/** Financial views invalidated by statement uploads and Trading 212 sync chunks. */
export function isImportedFinancialCacheKey(key: unknown): boolean {
  if (Array.isArray(key)) return key[0] === 'positions';
  if (typeof key !== 'string') return false;
  const path = key.split('?')[0];
  return importedEndpoints.some(endpoint => path === endpoint || path.startsWith(`${endpoint}/`));
}
