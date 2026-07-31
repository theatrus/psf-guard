import { useState, type FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import axios from 'axios';
import { apiClient } from '../api/client';
import type {
  AccessRole,
  ApiResponse,
  AuthUserSummary,
} from '../api/types';

interface UserManagementProps {
  currentUsername?: string;
}

type UserForm =
  | { kind: 'add' }
  | { kind: 'edit'; user: AuthUserSummary }
  | null;

function errorMessage(error: unknown): string {
  if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
    return error.response?.data?.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

export default function UserManagement({
  currentUsername,
}: UserManagementProps) {
  const queryClient = useQueryClient();
  const usersQuery = useQuery({
    queryKey: ['authUsers'],
    queryFn: apiClient.getAuthUsers,
  });
  const [form, setForm] = useState<UserForm>(null);
  const [username, setUsername] = useState('');
  const [role, setRole] = useState<AccessRole>('read_only');
  const [password, setPassword] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [status, setStatus] = useState('');
  const [saving, setSaving] = useState(false);

  const resetForm = () => {
    setForm(null);
    setUsername('');
    setRole('read_only');
    setPassword('');
    setConfirmation('');
  };

  const startAdd = () => {
    resetForm();
    setForm({ kind: 'add' });
    setStatus('');
  };

  const startEdit = (user: AuthUserSummary) => {
    setForm({ kind: 'edit', user });
    setUsername(user.username);
    setRole(user.role);
    setPassword('');
    setConfirmation('');
    setStatus('');
  };

  const publishUsers = (users: AuthUserSummary[]) => {
    queryClient.setQueryData(['authUsers'], users);
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (password !== confirmation) {
      setStatus('Passwords do not match.');
      return;
    }
    if (form?.kind === 'add' && password.length < 12) {
      setStatus('Password must be at least 12 characters.');
      return;
    }
    setSaving(true);
    setStatus('');
    try {
      if (form?.kind === 'add') {
        publishUsers(
          await apiClient.createAuthUser({ username: username.trim(), role, password })
        );
        setStatus('User added.');
      } else if (form?.kind === 'edit') {
        const request = {
          role,
          ...(password ? { password } : {}),
        };
        publishUsers(await apiClient.updateAuthUser(form.user.username, request));
        if (form.user.username === currentUsername) {
          await apiClient.logout();
          window.location.reload();
          return;
        }
        setStatus('User updated. Their existing sessions were signed out.');
      }
      resetForm();
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (user: AuthUserSummary) => {
    if (!confirm('Remove user "' + user.username + '"?')) return;
    setSaving(true);
    setStatus('');
    try {
      publishUsers(await apiClient.removeAuthUser(user.username));
      setStatus('User removed. Their existing sessions were signed out.');
      if (form?.kind === 'edit' && form.user.username === user.username) {
        resetForm();
      }
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="settings-section user-management">
      <div className="user-management-heading">
        <div>
          <h3>Browser users</h3>
          <p>
            Editors can change the catalog. Read-only users can review images
            and cached results without changing grades or settings.
          </p>
        </div>
        {!form && (
          <button
            type="button"
            className="add-directory-button"
            onClick={startAdd}
          >
            + Add user
          </button>
        )}
      </div>

      {usersQuery.isLoading && <div className="detecting-database">Loading users…</div>}
      {usersQuery.isError && (
        <div className="status-message error">
          {errorMessage(usersQuery.error)}
        </div>
      )}

      {usersQuery.data?.map((user) => {
        const isCurrent = user.username === currentUsername;
        return (
          <div className="user-row" key={user.username}>
            <div className="user-row-main">
              <div className="user-row-title">
                <strong>{user.username}</strong>
                {isCurrent && <span className="user-source-badge">Current</span>}
                {!user.managed && (
                  <span className="user-source-badge">TOML bootstrap</span>
                )}
              </div>
              <span className="muted">
                {user.role === 'read_write' ? 'Editor' : 'Read only'}
              </span>
            </div>
            {user.managed && (
              <div className="db-row-actions">
                <button
                  type="button"
                  className="browse-button"
                  onClick={() => startEdit(user)}
                  disabled={saving}
                >
                  Edit
                </button>
                <button
                  type="button"
                  className="remove-button"
                  onClick={() => void remove(user)}
                  disabled={saving || isCurrent}
                  title={
                    isCurrent
                      ? 'You cannot remove the account used by this session'
                      : 'Remove user'
                  }
                >
                  Remove
                </button>
              </div>
            )}
          </div>
        );
      })}

      {form && (
        <form className="user-form" onSubmit={(event) => void submit(event)}>
          <h3>{form.kind === 'add' ? 'Add user' : 'Edit user'}</h3>
          <div className="database-config">
            <label htmlFor="auth-username">Username</label>
            <input
              id="auth-username"
              className="file-path-input"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              disabled={form.kind === 'edit' || saving}
              autoComplete="username"
              required
            />
          </div>
          <div className="database-config">
            <label htmlFor="auth-role">Access</label>
            <select
              id="auth-role"
              className="file-path-input"
              value={role}
              onChange={(event) => setRole(event.target.value as AccessRole)}
              disabled={saving}
            >
              <option value="read_only">Read only</option>
              <option value="read_write">Editor</option>
            </select>
          </div>
          <div className="database-config">
            <label htmlFor="auth-password">
              {form.kind === 'add' ? 'Password' : 'New password (optional)'}
            </label>
            <input
              id="auth-password"
              className="file-path-input"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              autoComplete="new-password"
              minLength={form.kind === 'add' || password ? 12 : undefined}
              required={form.kind === 'add'}
              disabled={saving}
            />
          </div>
          <div className="database-config">
            <label htmlFor="auth-password-confirm">Confirm password</label>
            <input
              id="auth-password-confirm"
              className="file-path-input"
              type="password"
              value={confirmation}
              onChange={(event) => setConfirmation(event.target.value)}
              autoComplete="new-password"
              required={form.kind === 'add' || Boolean(password)}
              disabled={saving}
            />
          </div>
          {form.kind === 'edit' && form.user.username === currentUsername && (
            <p className="muted">
              Saving changes to your own account signs you out.
            </p>
          )}
          <div className="modal-buttons">
            <button type="submit" className="save-button" disabled={saving}>
              {saving ? 'Saving…' : 'Save user'}
            </button>
            <button
              type="button"
              className="cancel-button"
              onClick={resetForm}
              disabled={saving}
            >
              Cancel
            </button>
          </div>
        </form>
      )}

      {status && <div className="status-message">{status}</div>}

      <p className="muted user-management-note">
        TOML bootstrap accounts appear here but must be changed in the server
        config. User changes take effect at once.
      </p>
    </div>
  );
}
