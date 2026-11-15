import { useEffect, useRef, useState } from 'react';
import { apiClient } from '../../api/client';

/** A pairing code waiting to be used, for the Sync plugin's card and the
 *  card for other PSF Guard servers. The code goes when a client of that
 *  kind pairs, since that spends it, and when it expires. */
export function usePairingCode(databaseId: string, kind: 'plugin' | 'peer', pairedCount: number) {
  const [code, setCode] = useState<{ token: string; expiresAt: number } | null>(null);
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  // A client that pairs answers the code: it is spent.
  const seen = useRef(pairedCount);
  useEffect(() => {
    if (pairedCount > seen.current) setCode(null);
    seen.current = pairedCount;
  }, [pairedCount]);
  // An expired code can no longer pair: it goes, and so does the polling.
  useEffect(() => {
    if (!code) return;
    const timer = window.setTimeout(() => setCode(null), Math.max(0, code.expiresAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [code]);
  /** Ask the server for a new code; any earlier code of this kind stops working. */
  const issue = async () => {
    const issued = await apiClient.issuePairingCode(databaseId, kind);
    setCopy('idle');
    setCode({ token: issued.pairing_token, expiresAt: issued.expires_at * 1000 });
  };
  const copyCode = async () => {
    if (!code) return;
    try { await navigator.clipboard.writeText(code.token); setCopy('copied'); } catch { setCopy('failed'); }
  };
  return { code, copy, issue, copyCode };
}
