import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { OneTimeSecret } from '@/components/composite/account-secret';

export function RecoveryCodesScreen({ codes, onAcknowledge }: { codes: readonly string[]; onAcknowledge: () => void }) {
  const [saved, setSaved] = useState(false);
  const download = () => {
    const url = URL.createObjectURL(new Blob([`Doughbox recovery codes\n\nEach code can be used once with your password. Keep these offline and private.\n\n${codes.join('\n')}\n`], { type: 'text/plain;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'doughbox-recovery-codes.txt';
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return <main className="mx-auto flex min-h-svh max-w-lg items-center px-4 py-8">
    <Card className="w-full">
      <CardHeader><CardTitle>Save your recovery codes</CardTitle><CardDescription>This is the only time this set will be shown. Any previous recovery codes no longer work.</CardDescription></CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm">Each code replaces an authenticator code once, but you still need your password. Store them somewhere private, preferably offline and separate from your authenticator.</p>
        <OneTimeSecret title="Recovery codes" value={codes.join('\n')} />
        <Button type="button" variant="outline" onClick={download}>Download recovery codes</Button>
        <label className="flex items-start gap-3 text-sm"><input type="checkbox" className="mt-1 size-4 shrink-0" checked={saved} onChange={event => setSaved(event.target.checked)} />I have saved these recovery codes in a safe place.</label>
        <Button type="button" className="w-full" disabled={!saved} onClick={onAcknowledge}>Continue</Button>
      </CardContent>
    </Card>
  </main>;
}
