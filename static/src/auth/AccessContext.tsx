import {
  type FormEvent,
  type ReactNode,
  useEffect,
  useState,
} from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AuthStatus } from '../api/types';
import { AccessContext } from './access';
import { AUTH_REQUIRED_EVENT } from './events';

export default function AuthGate({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: ['authStatus'],
    queryFn: apiClient.getAuthStatus,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  const login = useMutation({
    mutationFn: ({ username, password }: { username: string; password: string }) =>
      apiClient.login(username, password),
    onSuccess: (status) => {
      queryClient.removeQueries({
        predicate: (query) => query.queryKey[0] !== 'authStatus',
      });
      queryClient.setQueryData(['authStatus'], status);
    },
  });

  useEffect(() => {
    const requireLogin = () => {
      queryClient.removeQueries({
        predicate: (query) => query.queryKey[0] !== 'authStatus',
      });
      queryClient.setQueryData<AuthStatus>(['authStatus'], {
        authentication_required: true,
        authenticated: false,
        can_compute: false,
      });
    };
    window.addEventListener(AUTH_REQUIRED_EVENT, requireLogin);
    return () => window.removeEventListener(AUTH_REQUIRED_EVENT, requireLogin);
  }, [queryClient]);

  if (statusQuery.isLoading) {
    return <div className="auth-page auth-loading">Checking access…</div>;
  }

  if (statusQuery.error) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <img src="/psf-guard.svg" alt="" className="auth-logo" />
          <h1>PSF Guard</h1>
          <p className="auth-error">Could not reach the server.</p>
          <button type="button" onClick={() => statusQuery.refetch()}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  const status = statusQuery.data!;
  if (status.authentication_required && !status.authenticated) {
    return (
      <LoginScreen
        busy={login.isPending}
        error={login.error instanceof Error ? login.error.message : undefined}
        onClearError={login.reset}
        onSubmit={(username, password) => login.mutate({ username, password })}
      />
    );
  }

  const effectiveStatus: AuthStatus = {
    authentication_required: status.authentication_required,
    authenticated: true,
    role: status.role ?? 'read_write',
    username: status.username,
    can_compute: status.can_compute,
  };
  return (
    <AccessContext.Provider
      value={{
        status: effectiveStatus,
        canWrite: effectiveStatus.role === 'read_write',
        canCompute: effectiveStatus.can_compute,
        logout: async () => {
          await apiClient.logout();
          queryClient.removeQueries({
            predicate: (query) => query.queryKey[0] !== 'authStatus',
          });
          queryClient.setQueryData<AuthStatus>(['authStatus'], {
            authentication_required: true,
            authenticated: false,
            can_compute: false,
          });
        },
      }}
    >
      {children}
    </AccessContext.Provider>
  );
}

function LoginScreen({
  busy,
  error,
  onClearError,
  onSubmit,
}: {
  busy: boolean;
  error?: string;
  onClearError: () => void;
  onSubmit: (username: string, password: string) => void;
}) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');

  const submit = (event: FormEvent) => {
    event.preventDefault();
    onSubmit(username, password);
  };

  return (
    <main className="auth-page">
      <form className="auth-card" onSubmit={submit}>
        <img src="/psf-guard.svg" alt="" className="auth-logo" />
        <h1>Sign in to PSF Guard</h1>
        <p>Use the viewer or editor account configured for this server.</p>
        <label>
          <span>Username</span>
          <input
            name="username"
            autoComplete="username"
            autoFocus
            value={username}
            onChange={(event) => {
              setUsername(event.target.value);
              onClearError();
            }}
            required
          />
        </label>
        <label>
          <span>Password</span>
          <input
            name="password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => {
              setPassword(event.target.value);
              onClearError();
            }}
            required
          />
        </label>
        {error && <p className="auth-error" role="alert">{error}</p>}
        <button type="submit" disabled={busy}>
          {busy ? 'Signing in…' : 'Sign in'}
        </button>
      </form>
    </main>
  );
}
