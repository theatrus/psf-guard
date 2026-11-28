import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import axios from 'axios';
import { apiClient } from '../api/client';
import type { ApiResponse } from '../api/types';

function errorMessage(error: unknown): string {
  if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
    return error.response?.data?.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

/**
 * The page PSF Studio opens in the browser when it connects to this
 * server: the person checks that the code matches the one PSF Studio shows,
 * and allows or denies the connection.
 */
export default function StudioConnectPage() {
  const [searchParams] = useSearchParams();
  const code = searchParams.get('code') ?? '';
  // A new code is a new connection: nothing decided for the last one
  // carries over.
  return <StudioConnect key={code} code={code} />;
}

function StudioConnect({ code }: { code: string }) {
  const [decided, setDecided] = useState<'allowed' | 'denied' | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const waiting = useQuery({
    queryKey: ['studioConnect', code],
    queryFn: () => apiClient.getStudioConnect(code),
    enabled: code !== '' && decided === null,
    retry: false,
    refetchInterval: false,
    staleTime: Number.POSITIVE_INFINITY,
  });

  const decide = async (allow: boolean) => {
    setBusy(true);
    setError('');
    try {
      await apiClient.decideStudioConnect(code, allow);
      setDecided(allow ? 'allowed' : 'denied');
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  };

  let body;
  if (decided === 'allowed') {
    body = (
      <>
        <h2>PSF Studio is connected</h2>
        <p>PSF Studio finishes connecting in a few seconds. You can close this tab.</p>
      </>
    );
  } else if (decided === 'denied') {
    body = (
      <>
        <h2>Connection denied</h2>
        <p>PSF Studio was not connected. You can close this tab.</p>
      </>
    );
  } else if (code === '') {
    body = (
      <>
        <h2>Connect PSF Studio</h2>
        <p className="auth-error">
          This address has no code. In PSF Studio, choose File › Connect to PSF Guard… to start
          again.
        </p>
      </>
    );
  } else if (waiting.isLoading) {
    body = <p>Looking for the code…</p>;
  } else if (waiting.isError || !waiting.data) {
    body = (
      <>
        <h2>Connect PSF Studio</h2>
        <p className="auth-error">{errorMessage(waiting.error)}</p>
      </>
    );
  } else {
    const details = waiting.data;
    body = (
      <>
        <h2>Connect PSF Studio</h2>
        <p>
          PSF Studio on <strong>{details.client_name}</strong> asks to connect to this server
          as you.
        </p>
        <p>Check that PSF Studio shows this code:</p>
        <p className="studio-connect-code" aria-label="Code">
          {details.user_code}
        </p>
        <p>
          {details.saves_documents
            ? 'PSF Studio will read projects, targets and stacks, and save Studio documents and their output to projects. It cannot grade frames, change plans or manage databases.'
            : 'PSF Studio will read projects, targets and stacks. Your account is read-only, so PSF Studio saves documents on its own computer only.'}
        </p>
        <p className="muted">
          The connection lasts 90 days. Revoke it at any time in Settings › API tokens.
        </p>
        {error && <p className="auth-error">{error}</p>}
        <div className="studio-connect-actions">
          <button type="button" disabled={busy} onClick={() => void decide(true)}>
            Allow
          </button>
          <button
            type="button"
            className="cancel-button"
            disabled={busy}
            onClick={() => void decide(false)}
          >
            Deny
          </button>
        </div>
      </>
    );
  }

  return (
    <div className="studio-connect-page">
      <div className="auth-card" role="region" aria-label="Connect PSF Studio">
        {body}
      </div>
    </div>
  );
}
