import { useState } from 'react';
import { claudeCommand, codexConfig, TOKEN_VARIABLE } from '../utils/agentSetup';

function Snippet({ title, text, note }: { title: string; text: string; note?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="agent-snippet">
      <div className="agent-snippet-heading">
        <strong>{title}</strong>
        <button type="button" className="browse-button" onClick={() => void copy()} aria-label={`Copy the ${title} setup`}>
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre>{text}</pre>
      {note && <small className="muted">{note}</small>}
    </div>
  );
}

/**
 * What to paste into Claude Code or Codex to connect it to this server's MCP
 * endpoint. With a token, the commands carry it; without one (a server with
 * no accounts, on this machine) they need none.
 */
export default function AgentConnect({ url, token }: { url: string; token?: string }) {
  if (!url) return null;
  return (
    <div className="agent-connect" aria-label="Connect an agent">
      <Snippet title="Claude Code" text={claudeCommand(url, token)} />
      <Snippet
        title="Codex"
        text={codexConfig(url, token)}
        note={token ? `Codex reads the token from ${TOKEN_VARIABLE}: export ${TOKEN_VARIABLE}=${token} in the shell you start it from.` : undefined}
      />
    </div>
  );
}
