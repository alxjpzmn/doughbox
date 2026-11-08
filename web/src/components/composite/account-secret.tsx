import { useId, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';

export function OneTimeSecret({ title, value, onHide }: { title: string; value: string; onHide?: () => void }) {
  const id = useId();
  const [hidden, setHidden] = useState(false);
  const [feedback, setFeedback] = useState('');
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setFeedback('Copied. Keep this secret somewhere safe.');
    } catch {
      setFeedback('Automatic copy is unavailable. Select the text below and copy it manually.');
    }
  };
  return <section className="min-w-0 space-y-2 rounded-md border p-3" aria-label={title}>
    <Label htmlFor={id}>{title}</Label>
    {!hidden && <textarea id={id} readOnly value={value} rows={value.includes('\n') ? Math.min(value.split('\n').length, 12) : 3}
      className="w-full resize-y rounded-md border bg-background p-2 font-mono text-sm break-all" onFocus={event => event.currentTarget.select()} spellCheck={false} />}
    <div className="flex flex-wrap gap-2">
      {!hidden && <Button type="button" size="sm" variant="outline" onClick={copy}>Copy</Button>}
      {!hidden && <Button type="button" size="sm" variant="outline" onClick={() => { setHidden(true); setFeedback(''); onHide?.(); }}>Hide</Button>}
    </div>
    {hidden && <p className="text-sm text-muted-foreground">Secret hidden. It cannot be shown again here.</p>}
    {feedback && <p role="status" className="text-sm text-muted-foreground">{feedback}</p>}
  </section>;
}
