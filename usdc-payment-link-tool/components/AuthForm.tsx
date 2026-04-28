'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';

type Props = {
  mode: 'login' | 'register';
};

export function AuthForm({ mode }: Props) {
  const router = useRouter();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  async function handleSubmit(formData: FormData) {
    setLoading(true);
    setError('');
    const body = Object.fromEntries(formData.entries());
    const endpoint = mode === 'login' ? '/api/auth/login' : '/api/auth/register';
    const res = await fetch(endpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-correlation-id': crypto.randomUUID()
      },
      body: JSON.stringify(body),
    });
    const data = await res.json();
    setLoading(false);
    if (!res.ok) {
      setError(data.error || 'Request failed');
      return;
    }
    router.push('/dashboard');
    router.refresh();
  }

  return (
    <form action={handleSubmit} className="card stack">
      <h1>{mode === 'login' ? 'Sign in to ASTROpay' : 'Create your ASTROpay merchant account'}</h1>
      {mode === 'register' ? <>
        <label><span>Business name</span><input name="businessName" className="input" required /></label>
        <label><span>Stellar public key</span><input name="stellarPublicKey" className="input" required /></label>
        <label><span>Settlement public key</span><input name="settlementPublicKey" className="input" required /></label>
      </> : null}
      <label><span>Email</span><input type="email" name="email" className="input" required /></label>
      <label><span>Password</span><input type="password" name="password" className="input" required /></label>
      {error ? <p className="error">{error}</p> : null}
      <button className="button" disabled={loading}>{loading ? 'Working...' : mode === 'login' ? 'Sign in' : 'Create account'}</button>
    </form>
  );
}
