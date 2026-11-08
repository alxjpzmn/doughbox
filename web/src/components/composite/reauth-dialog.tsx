import { useEffect, useRef, useState } from 'react';
import { useForm } from 'react-hook-form';
import { yupResolver } from '@hookform/resolvers/yup';
import * as yup from 'yup';
import { CodeType, type ReauthRequest } from '@/types/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

const schema = yup.object({
  password: yup.string().required('Enter your current password.'),
  code: yup.string().required('Enter an authenticator or recovery code.'),
}).required();
type Fields = yup.InferType<typeof schema>;

export function ReauthenticationDialog({ open, onOpenChange, onSubmit }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmit: (input: ReauthRequest) => Promise<void>;
}) {
  const [codeType, setCodeType] = useState(CodeType.Totp);
  const [error, setError] = useState('');
  const generation = useRef(0);
  const submitting = useRef(false);
  const { register, handleSubmit, reset, formState: { errors, isSubmitting } } = useForm<Fields>({ resolver: yupResolver(schema), defaultValues: { password: '', code: '' } });
  useEffect(() => {
    generation.current += 1;
    reset();
    setError('');
    setCodeType(CodeType.Totp);
    return () => { generation.current += 1; };
  }, [open, reset]);
  const submit = async (fields: Fields) => {
    if (submitting.current) return;
    if (codeType === CodeType.Totp && !/^\d{6}$/.test(fields.code)) { setError('Enter the six-digit code from your authenticator.'); return; }
    const request = generation.current;
    submitting.current = true;
    setError('');
    try {
      await onSubmit({ ...fields, code_type: codeType });
      if (request === generation.current) { reset(); onOpenChange(false); }
    } catch (failure) {
      if (request === generation.current && !(failure instanceof Error && failure.name === 'AbortError')) {
        setError(failure && typeof failure === 'object' && 'message' in failure ? String(failure.message) : 'Verification failed. Please try again.');
      }
    } finally { submitting.current = false; }
  };
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent><DialogHeader><DialogTitle>Verify it’s you</DialogTitle><DialogDescription>Confirm your password and second factor for five minutes of sensitive account access. After verification, retry your original action explicitly; it will not run automatically.</DialogDescription></DialogHeader>
      <form onSubmit={handleSubmit(submit)} className="space-y-4">
        <div className="space-y-2"><Label htmlFor="reauth-password">Current password</Label><Input id="reauth-password" type="password" autoComplete="current-password" {...register('password')} aria-invalid={!!errors.password} disabled={isSubmitting} />{errors.password && <p role="alert" className="text-sm text-destructive-foreground">{errors.password.message}</p>}</div>
        <div className="space-y-2"><Label htmlFor="reauth-code">{codeType === CodeType.Totp ? 'Six-digit authenticator code' : 'Unused recovery code'}</Label><Input id="reauth-code" type="text" inputMode={codeType === CodeType.Totp ? 'numeric' : 'text'} autoComplete="one-time-code" {...register('code')} aria-invalid={!!errors.code} disabled={isSubmitting} />{errors.code && <p role="alert" className="text-sm text-destructive-foreground">{errors.code.message}</p>}</div>
        <Button type="button" variant="link" className="h-auto p-0" disabled={isSubmitting} onClick={() => { setCodeType(codeType === CodeType.Totp ? CodeType.Recovery : CodeType.Totp); reset({ password: '', code: '' }); setError(''); }}>{codeType === CodeType.Totp ? 'Use a recovery code' : 'Use an authenticator code'}</Button>
        {error && <p role="alert" className="text-sm text-destructive-foreground">{error}</p>}
        <Button type="submit" disabled={isSubmitting} className="w-full">{isSubmitting ? 'Verifying…' : 'Verify identity'}</Button>
      </form>
    </DialogContent>
  </Dialog>;
}
