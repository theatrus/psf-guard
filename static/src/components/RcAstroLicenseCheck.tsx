import { useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import type { RcAstroLicenseCheckStatus } from '../api/types';

/**
 * A button that asks RC-Astro, online, for one computer's account and
 * license state, and the report it gives. The check is a job on the server:
 * the click starts it, this polls it once a second until it ends, and Stop
 * ends it. It runs only when clicked, since RC-Astro asks hosts not to
 * check routinely. Diagnostics only; nothing here activates or deactivates
 * a license.
 */
export default function RcAstroLicenseCheck({
  where,
  start,
  read,
  stop,
}: {
  /** The computer checked, for the button's name: "this server" or a
   *  worker's name. */
  where: string;
  start: () => Promise<RcAstroLicenseCheckStatus>;
  read: (key: string) => Promise<RcAstroLicenseCheckStatus>;
  stop: (key: string) => Promise<void>;
}) {
  const [started, setStarted] = useState<RcAstroLicenseCheckStatus | null>(null);
  const begin = useMutation({ mutationFn: () => start(), onSuccess: setStarted });
  const polled = useQuery({
    queryKey: ['rc-astro-license-check', started?.key, started?.started_at],
    queryFn: () => read(started!.key),
    enabled: !!started?.running,
    refetchInterval: (query) => (query.state.data?.running === false ? false : 1000),
  });
  const halt = useMutation({ mutationFn: (key: string) => stop(key) });
  const status = polled.data ?? started;
  const running = begin.isPending || !!status?.running;
  const check = status?.check;
  const report = check?.report;
  const failed = begin.error ?? polled.error;
  return (
    <div className="rc-astro-license" role="group" aria-label={`RC-Astro license on ${where}`}>
      <div className="rc-astro-license-actions">
        <button type="button" className="browse-button" disabled={running}
          aria-label={`Check license on ${where}`}
          onClick={() => begin.mutate()}>
          {running ? 'Checking…' : 'Check license'}
        </button>
        {status?.running && (
          <button type="button" className="browse-button" disabled={halt.isPending}
            aria-label={`Stop the license check on ${where}`}
            onClick={() => halt.mutate(status.key)}>
            Stop
          </button>
        )}
      </div>
      {failed && <p className="error-text" role="alert">{failed.message}</p>}
      {status && !status.running && (
        <div className="rc-astro-license-report" role="status">
          {status.problem && <p className="error-text">{status.problem}</p>}
          {check && (
            <p>
              rc-astro: <code>{check.executable}</code>
              {check.cli_version ? ` · ${check.cli_version}` : ''}
            </p>
          )}
          {report && (
            <>
              <p>Computer: {report.computer}</p>
              <p>
                {report.account_email
                  ? `RC-Astro account: ${report.account_email}`
                  : 'This computer is not registered to an RC-Astro account.'}
                {!report.account_email && report.activation_url && (
                  <> <a href={report.activation_url} target="_blank" rel="noreferrer">Get an activation code</a></>
                )}
              </p>
              {!!report.products.length && (
                <ul>
                  {report.products.map((product) => (
                    <li key={product.product} data-status={product.status}>
                      <strong>{product.name}</strong>: {product.message}
                    </li>
                  ))}
                </ul>
              )}
              {report.attention && (
                <p data-attention={report.attention.reason}>
                  {report.attention.message}
                  {report.attention.command && (
                    <> Run in a terminal on that computer: <code>{report.attention.command}</code></>
                  )}
                </p>
              )}
            </>
          )}
          {/* RC-Astro's words are marked as RC-Astro's; PSF Guard's own
              failure to get an answer is said plainly. */}
          {check?.error && <p className="error-text">RC-Astro reported: {check.error}</p>}
          {check?.failure && <p className="error-text">{check.failure}</p>}
          {!report && <p>The license state each tool reports is unchanged.</p>}
        </div>
      )}
    </div>
  );
}
