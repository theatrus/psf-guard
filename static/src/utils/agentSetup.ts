import { useEffect, useState } from 'react';
import { getServerUrl, isTauriApp } from './tauri';

/** This server's MCP endpoint, as an agent on another machine (or this one,
 *  for the desktop app) reaches it. */
export function useMcpUrl(): string {
  const [base, setBase] = useState(() => (isTauriApp() ? '' : window.location.origin));
  useEffect(() => {
    if (!isTauriApp()) return;
    let live = true;
    void getServerUrl().then((url) => { if (live) setBase(url); });
    return () => { live = false; };
  }, []);
  return base ? `${base}/api/mcp` : '';
}

/** The environment variable Codex reads the token from. */
export const TOKEN_VARIABLE = 'PSF_GUARD_TOKEN';

export function claudeCommand(url: string, token?: string): string {
  const header = token ? ` --header "Authorization: Bearer ${token}"` : '';
  return `claude mcp add --transport http psf-guard ${url}${header}`;
}

export function codexConfig(url: string, token?: string): string {
  const lines = ['# ~/.codex/config.toml', '[mcp_servers.psf-guard]', `url = "${url}"`];
  if (token) lines.push(`bearer_token_env_var = "${TOKEN_VARIABLE}"`);
  return lines.join('\n');
}
