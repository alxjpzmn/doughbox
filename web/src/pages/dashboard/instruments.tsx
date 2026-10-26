import { useRef, useState, type FormEvent } from 'react';
import useSwr, { useSWRConfig } from 'swr';
import { InstrumentDetails, PricingStatus } from '@/types/core';
import { BASE_URL, fetcher, sendMutateRequest } from '@/lib/http';
import { PriceStatus } from '@/components/composite/price-status';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Skeleton } from '@/components/ui/skeleton';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';

const catalogueUrl = `${BASE_URL}/instruments`;
const affectedEndpoints = [
  'instruments', 'prices', 'portfolio', 'positions', 'past_performance',
  'performance', 'performance_overview', 'timeline', 'taxation',
].map(endpoint => `${BASE_URL}/${endpoint}`);

const sourceLabels: Record<string, string> = {
  deutsche_boerse: 'Deutsche Börse',
  cboe: 'Cboe Europe',
  euronext: 'Euronext',
  execution_fallback: 'Last imported execution',
  legacy_unknown: 'Existing stored price',
  manual: 'Manual override',
  missing: 'Missing',
  unavailable: 'Unavailable',
};

const errorMessage = (error: unknown): string => {
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
    return error.message;
  }
  return 'A network error occurred. Please try again.';
};

type SaveAction = 'name' | 'price' | 'unpin';
type Feedback = { error: boolean; message: string };

const InstrumentCard = ({ instrument }: { instrument: InstrumentDetails }) => {
  const { mutate } = useSWRConfig();
  const [name, setName] = useState(instrument.name);
  const [price, setPrice] = useState('');
  const [asOf, setAsOf] = useState('');
  const [saving, setSaving] = useState<SaveAction | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const submitting = useRef(false);
  const detailUrl = `${catalogueUrl}/${encodeURIComponent(instrument.id)}`;
  const quote = instrument.quote;
  const fieldId = `instrument-${instrument.id}`;

  const save = async (action: SaveAction, payload?: { name: string } | { price: string; as_of?: string }) => {
    if (submitting.current) return;
    submitting.current = true;
    setSaving(action);
    setFeedback(null);

    try {
      let updated: InstrumentDetails;
      try {
        const response = await sendMutateRequest(
          action === 'name' ? detailUrl : `${detailUrl}/price`,
          payload,
          { method: action === 'name' ? 'PATCH' : action === 'price' ? 'PUT' : 'DELETE' },
        );
        updated = await response.json();
      } catch (error: unknown) {
        setFeedback({ error: true, message: errorMessage(error) });
        return;
      }

      if (action === 'name') setName(updated.name);
      const successMessage = action === 'name' ? 'Name saved. Prices are unchanged.'
        : action === 'price' ? 'Manual EUR price pinned. Automatic prices will not replace it until you remove the pin.'
          : 'Manual pin removed. The best eligible stored price or fallback is now used; the price may be unavailable.';
      setFeedback({ error: false, message: successMessage });

      try {
        await mutate<InstrumentDetails[]>(catalogueUrl, current => current?.map(item => item.id === updated.id ? updated : item), { revalidate: false });
        await mutate(detailUrl, updated, { revalidate: false });
        await mutate(key => {
          if (typeof key !== 'string') return false;
          const path = key.split('?')[0];
          return affectedEndpoints.some(endpoint => path === endpoint || path.startsWith(`${endpoint}/`));
        });
      } catch (error: unknown) {
        setFeedback({ error: true, message: `${successMessage} Related data could not be refreshed. Reload to see current values. ${errorMessage(error)}` });
      }
    } finally {
      submitting.current = false;
      setSaving(null);
    }
  };

  const saveName = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!name.trim()) {
      setFeedback({ error: true, message: 'Enter a non-blank instrument name.' });
      return;
    }
    void save('name', { name: name.trim() });
  };

  const savePrice = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const exactPrice = price.trim();
    if (!/^(?:\d+(?:\.\d*)?|\.\d+)$/.test(exactPrice) || !/[1-9]/.test(exactPrice)) {
      setFeedback({ error: true, message: 'Enter a positive EUR unit price using a decimal point, without currency symbols or grouping separators.' });
      return;
    }
    let timestamp: string | undefined;
    if (asOf) {
      const date = new Date(asOf);
      if (!Number.isFinite(date.getTime()) || date.getTime() > Date.now()) {
        setFeedback({ error: true, message: 'Choose a valid price time that is not in the future, or leave it blank to use server time.' });
        return;
      }
      timestamp = date.toISOString();
    }
    void save('price', { price: exactPrice, ...(timestamp ? { as_of: timestamp } : {}) });
  };

  return (
    <Card>
      <CardHeader className="pb-3">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0 flex-1">
            <CardTitle className="break-words text-lg">{instrument.name}</CardTitle>
            <CardDescription className="mt-1 break-all font-mono">{instrument.id}</CardDescription>
          </div>
          <Badge variant={quote.pinned ? 'secondary' : 'outline'}>{quote.pinned ? 'Pinned manually' : 'Not pinned'}</Badge>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div>
          <p className="break-all text-xl font-semibold tabular-nums">{quote.price == null ? 'Unavailable · EUR' : `${quote.price} EUR`}</p>
          <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
            <dt className="text-muted-foreground">Source</dt>
            <dd className="break-words">{sourceLabels[quote.source] ?? quote.source}</dd>
            <dt className="text-muted-foreground">Venue</dt>
            <dd className="break-words">{quote.venue || 'Not available'}</dd>
            <dt className="text-muted-foreground">As of</dt>
            <dd>{quote.as_of ? new Date(String(quote.as_of)).toLocaleString() : 'Not available'}{quote.older_than_hour && ' · Older than 1h'}</dd>
          </dl>
        </div>
        <details className="border-t pt-3">
          <summary className="min-h-11 cursor-pointer py-2 text-sm font-medium">Manage instrument<span className="sr-only"> {instrument.name} ({instrument.id})</span></summary>
          <div className="space-y-5 pt-3">
            <p className="text-sm text-muted-foreground">ISIN: <span className="font-mono">{instrument.id}</span> (read-only). Only existing imported instruments can be managed here.</p>
            <form onSubmit={saveName}>
              <fieldset disabled={saving !== null} className="min-w-0 space-y-3">
                <legend className="mb-3 font-medium">Instrument name</legend>
                <Label htmlFor={`${fieldId}-name`}>Name</Label>
                <Input id={`${fieldId}-name`} value={name} onChange={event => setName(event.target.value)} required className="min-h-11" />
                <Button type="submit" className="min-h-11" disabled={saving !== null || name === instrument.name}>
                  {saving === 'name' ? 'Saving name…' : 'Save name'}
                </Button>
                <p className="text-xs text-muted-foreground">Renaming does not change or pin the price.</p>
              </fieldset>
            </form>
            <form onSubmit={savePrice} className="border-t pt-4">
              <fieldset disabled={saving !== null} className="min-w-0 space-y-3">
                <legend className="mb-3 font-medium">Manual EUR price</legend>
                <Label htmlFor={`${fieldId}-price`}>EUR price per unit</Label>
                <Input id={`${fieldId}-price`} type="text" inputMode="decimal" autoComplete="off" placeholder="123.456789" value={price} onChange={event => setPrice(event.target.value)} required aria-describedby={`${fieldId}-price-help`} className="min-h-11" />
                <p id={`${fieldId}-price-help`} className="text-xs text-muted-foreground">Use a decimal point. All entered decimal places are preserved.</p>
                <Label htmlFor={`${fieldId}-as-of`}>Price time (optional, local time)</Label>
                <Input id={`${fieldId}-as-of`} type="datetime-local" step="1" value={asOf} onChange={event => setAsOf(event.target.value)} aria-describedby={`${fieldId}-time-help`} className="min-h-11 min-w-0" />
                <p id={`${fieldId}-time-help`} className="text-xs text-muted-foreground">Leave blank to use the server's current time. Future times are not allowed.</p>
                <p className="text-sm text-muted-foreground">Pinning overrides automatic valuation for this instrument until you remove the pin. The instrument name is unchanged.</p>
                <Button type="submit" className="min-h-11" disabled={saving !== null}>{saving === 'price' ? 'Saving price…' : 'Save and pin EUR price'}</Button>
              </fieldset>
            </form>
            {quote.pinned && <div className="space-y-3 border-t pt-4">
              <p className="text-sm text-muted-foreground">Remove the manual pin to use the best eligible stored market price or execution fallback again. This does not request a live quote; the resulting price may be unavailable.</p>
              <Button type="button" variant="outline" className="min-h-11" disabled={saving !== null} onClick={() => void save('unpin')}>
                {saving === 'unpin' ? 'Removing pin…' : 'Remove manual pin'}
              </Button>
            </div>}
          </div>
        </details>
        {feedback && <p role={feedback.error ? 'alert' : 'status'} className={`break-words text-sm ${feedback.error ? 'text-destructive' : 'text-muted-foreground'}`}>{feedback.message}</p>}
      </CardContent>
    </Card>
  );
};

const Instruments = () => {
  const [search, setSearch] = useState('');
  const [activeOnly, setActiveOnly] = useState(false);
  const { data, error, isLoading, isValidating, mutate } = useSwr<InstrumentDetails[]>(catalogueUrl, fetcher, { refreshInterval: 60_000 });
  const { data: pricingStatus, error: pricingError } = useSwr<PricingStatus>(`${BASE_URL}/prices`, fetcher, { refreshInterval: 60_000 });
  // Price status includes every current holding, even when its valuation is unavailable.
  const activeIds = new Set(pricingStatus?.quotes.map(quote => quote.isin));
  const query = search.trim().toLocaleLowerCase();
  const matchingIds = new Set(data?.filter(instrument =>
    (!activeOnly || activeIds.has(instrument.id))
    && (instrument.name.toLocaleLowerCase().includes(query) || instrument.id.toLocaleLowerCase().includes(query))
  ).map(instrument => instrument.id));
  const matchCount = matchingIds.size;

  return (
    <Tabs defaultValue="price-feeds" className="pb-32 md:pb-0">
      <TabsList className="mb-6" aria-label="Data sections">
        <TabsTrigger value="price-feeds">Price Feeds</TabsTrigger>
        <TabsTrigger value="instrument-management">Instruments</TabsTrigger>
      </TabsList>

      <TabsContent value="price-feeds" forceMount className="data-[state=inactive]:hidden">
        <PriceStatus />
      </TabsContent>

      <TabsContent value="instrument-management" forceMount className="space-y-6 data-[state=inactive]:hidden">
        <Card>
          <CardHeader>
            <CardTitle>Instruments</CardTitle>
            <CardDescription>Search and manage all imported instruments, including those no longer held. Prices shown here are safe valuation quotes, not necessarily live market prices.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Label htmlFor="instrument-search">Search by name or ISIN</Label>
            <Input id="instrument-search" type="search" value={search} onChange={event => setSearch(event.target.value)} placeholder="Name or ISIN" className="min-h-11" />
            <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
              {data && <p role="status" className="text-sm text-muted-foreground">{matchCount} of {data.length} imported instruments{activeOnly && ' · active only'}</p>}
              <div className="flex min-h-11 items-center gap-2">
                <Switch id="active-instruments" checked={activeOnly} onCheckedChange={setActiveOnly} disabled={!pricingStatus} aria-describedby="active-instruments-help" />
                <Label htmlFor="active-instruments" className="flex min-h-11 cursor-pointer items-center">Active only</Label>
              </div>
            </div>
            <p id="active-instruments-help" className="text-xs text-muted-foreground">Active instruments are those you currently hold, whether or not a price is available.</p>
            {!pricingStatus && !pricingError && <p role="status" className="text-sm text-muted-foreground">Loading current holdings…</p>}
            {pricingError && <p role="alert" className="text-sm text-destructive">{pricingStatus ? 'Current holdings could not be refreshed. The active filter uses the last loaded holdings.' : 'The active filter is unavailable because current holdings could not be loaded.'}</p>}
          </CardContent>
        </Card>

        {error && <Card><CardContent className="space-y-3 pt-6">
          <p role="alert" className="break-words text-sm text-destructive">{data ? 'The catalogue could not be refreshed. Previously loaded instruments are shown. ' : 'The instrument catalogue could not be loaded. '}{errorMessage(error)}</p>
          <Button type="button" variant="outline" className="min-h-11" disabled={isValidating} onClick={() => void mutate().catch(() => undefined)}>{isValidating ? 'Retrying…' : 'Retry catalogue'}</Button>
        </CardContent></Card>}

        {isLoading && !data && <div role="status" aria-label="Loading instruments" className="space-y-4">
          {[0, 1, 2].map(index => <Card key={index}><CardContent className="space-y-3 pt-6"><Skeleton className="h-6 w-2/3" /><Skeleton className="h-4 w-1/3" /><Skeleton className="h-8 w-1/2" /></CardContent></Card>)}
        </div>}

        {data?.length === 0 && <Card><CardContent className="pt-6"><p className="text-sm text-muted-foreground">No imported instruments yet. Instruments appear here after importing your investment data.</p></CardContent></Card>}
        {data && data.length > 0 && matchCount === 0 && <Card><CardContent className="space-y-3 pt-6">
          <p className="text-sm text-muted-foreground">{activeOnly ? (query ? 'No active instruments match your search.' : 'You do not currently hold any imported instruments.') : 'No instruments match your search.'}</p>
          <div className="flex flex-wrap gap-2">
            {query && <Button type="button" variant="outline" className="min-h-11" onClick={() => setSearch('')}>Clear search</Button>}
            {activeOnly && <Button type="button" variant="outline" className="min-h-11" onClick={() => setActiveOnly(false)}>Show all instruments</Button>}
          </div>
        </CardContent></Card>}

        {data && data.length > 0 && <div className="space-y-4">
          {data.map(instrument => <div key={instrument.id} hidden={!matchingIds.has(instrument.id)}><InstrumentCard instrument={instrument} /></div>)}
        </div>}
      </TabsContent>
    </Tabs>
  );
};

export default Instruments;
