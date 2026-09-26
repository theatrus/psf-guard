import { useState, type FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import axios from 'axios';
import { apiClient } from '../api/client';
import { describeTokenExpiry, formatTokenDate } from '../utils/apiTokens';
import type { ApiResponse, AuthTokenSummary, MintedAuthToken } from '../api/types';

interface ApiTokenManagementProps {
  currentUsername?: string;
  /** Editors see every token and can mint for other users. */
  isEditor: boolean;
}

function errorMessage(error: unknown): string {
  if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
    return error.response?.data?.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

export default function ApiTokenManagement({
  currentUsername,
  isEditor,
}: ApiTokenManagementProps) {
  const queryClient = useQueryClient();
  const tokensQuery = useQuery({
    queryKey: ['authTokens'],
    queryFn: apiClient.getAuthTokens,
  });
  const [formOpen, setFormOpen] = useState(false);
  const [label, setLabel] = useState('');
  const [username, setUsername] = useState('');
  const [readOnly, setReadOnly] = useState(false);
  const [expiresInDays, setExpiresInDays] = useState('');
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState('');
  const [minted, setMinted] = useState<MintedAuthToken | null>(null);
  const [copied, setCopied] = useState(false);

  const publish = (tokens: AuthTokenSummary[]) => {
    queryClient.setQueryData(['authTokens'], tokens);
  };

  const resetForm = () => {
    setFormOpen(false);
    setLabel('');
    setUsername('');
    setReadOnly(false);
    setExpiresInDays('');
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const days = expiresInDays.trim() ? Number(expiresInDays) : undefined;
    if (days !== undefined && (!Number.isInteger(days) || days < 1)) {
      setStatus('Expiry must be a whole number of days.');
      return;
    }
    setSaving(true);
    setStatus('');
    try {
      const result = await apiClient.createAuthToken({
        label: label.trim(),
        read_only: readOnly,
        expires_in_days: days,
        username: isEditor && username.trim() ? username.trim() : undefined,
      });
      publish(result.tokens);
      setMinted(result);
      setCopied(false);
      resetForm();
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const revoke = async (token: AuthTokenSummary) => {
    if (!window.confirm(`Revoke the token "${token.label}"? Anything using it stops at once.`)) {
      return;
    }
    setSaving(true);
    setStatus('');
    try {
      publish(await apiClient.revokeAuthToken(token.id));
      if (minted?.summary.id === token.id) setMinted(null);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const copySecret = async () => {
    if (!minted) return;
    try {
      await navigator.clipboard.writeText(minted.token);
      setCopied(true);
    } catch {
      setStatus('Copy failed; select the token and copy it by hand.');
    }
  };

  return (
    <div className="settings-section user-management api-tokens">
      <div className="user-management-heading">
        <div>
          <h3>API tokens</h3>
          <p>
            A token lets a script or an MCP client such as Claude act as{' '}
            {isEditor ? 'a user' : 'you'} without a browser login. Send it as{' '}
            <code>Authorization: Bearer …</code>. The MCP endpoint is{' '}
            <code>/api/mcp</code>.
          </p>
        </div>
        {!formOpen && (
          <button
            type="button"
            className="add-directory-button"
            onClick={() => {
              setFormOpen(true);
              setMinted(null);
              setStatus('');
            }}
          >
            + New token
          </button>
        )}
      </div>

      {minted && (
        <div className="token-secret" role="status">
          <p>
            <strong>Copy this token now.</strong> It is not shown again.
          </p>
          <div className="token-secret-row">
            <code className="token-secret-value">{minted.token}</code>
            <button type="button" className="browse-button" onClick={() => void copySecret()}>
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <button type="button" className="cancel-button" onClick={() => setMinted(null)}>
            Done
          </button>
        </div>
      )}

      {tokensQuery.isLoading && <div className="detecting-database">Loading tokens…</div>}
      {tokensQuery.isError && (
        <div className="status-message error">{errorMessage(tokensQuery.error)}</div>
      )}
      {tokensQuery.data?.length === 0 && !formOpen && (
        <p className="muted">No tokens yet.</p>
      )}

      {tokensQuery.data?.map((token) => (
        <div className="user-row token-row" key={token.id}>
          <div className="user-row-main">
            <div className="user-row-title">
              <strong>{token.label}</strong>
              {token.username !== currentUsername && (
                <span className="user-source-badge">{token.username}</span>
              )}
            </div>
            <div className="user-row-details">
              <span className="muted">
                {token.role === 'read_write' ? 'Editor' : 'Read only'}
              </span>
              <span className="muted">Created {formatTokenDate(token.created_at)}</span>
              <span className="muted">{describeTokenExpiry(token)}</span>
              <span className="muted">id {token.id}</span>
            </div>
          </div>
          <div className="db-row-actions">
            <button
              type="button"
              className="remove-button"
              onClick={() => void revoke(token)}
              disabled={saving}
            >
              Revoke
            </button>
          </div>
        </div>
      ))}

      {formOpen && (
        <form className="user-form" onSubmit={(event) => void submit(event)}>
          <h3>New token</h3>
          <div className="database-config">
            <label htmlFor="token-label">Label</label>
            <input
              id="token-label"
              className="file-path-input"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              placeholder="claude on laptop"
              maxLength={80}
              required
              disabled={saving}
            />
          </div>
          {isEditor && (
            <div className="database-config">
              <label htmlFor="token-username">User (optional)</label>
              <input
                id="token-username"
                className="file-path-input"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                placeholder={currentUsername ?? ''}
                disabled={saving}
              />
            </div>
          )}
          <div className="database-config">
            <label htmlFor="token-expiry">Expires after (days, optional)</label>
            <input
              id="token-expiry"
              className="file-path-input"
              type="number"
              min={1}
              max={3650}
              value={expiresInDays}
              onChange={(event) => setExpiresInDays(event.target.value)}
              disabled={saving}
            />
          </div>
          <div className="database-config">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={readOnly}
                onChange={(event) => setReadOnly(event.target.checked)}
                disabled={saving}
              />{' '}
              Read only, whatever the user's role
            </label>
          </div>
          <div className="modal-buttons">
            <button type="submit" className="save-button" disabled={saving}>
              {saving ? 'Creating…' : 'Create token'}
            </button>
            <button type="button" className="cancel-button" onClick={resetForm} disabled={saving}>
              Cancel
            </button>
          </div>
        </form>
      )}

      {status && <div className="status-message">{status}</div>}
    </div>
  );
}
