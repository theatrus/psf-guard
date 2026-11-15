import { useMutation } from '@tanstack/react-query';
import type { RcAstroLicenseCheck as Check } from '../api/types';

/**
 * A button that asks RC-Astro, online, for one computer's account and
 * license state, and the report it gives. It runs only when clicked:
 * RC-Astro asks hosts not to check routinely. Diagnostics only; nothing
 * here activates or deactivates a license.
 */
export default function RcAstroLicenseCheck({
  where,
  check,
}: {
  /** The computer checked, for the button's name: "this server" or a
   *  worker's name. */
  where: string;
  check: () => Promise<Check>;
}) {
  const run = useMutation({ mutationFn: check });
  const result = run.data;
  const report = result?.report;
  return (
    <div className="rc-astro-license" role="group" aria-label={`RC-Astro license on ${where}`}>
      <button type="button" className="browse-button" disabled={run.isPending}
        aria-label={`Check license on ${where}`}
        onClick={() => run.mutate()}>
        {run.isPending ? 'Checking…' : 'Check license'}
      </button>
      {run.error && <p className="error-text" role="alert">{run.error.message}</p>}
      {result && (
        <div className="rc-astro-license-report" role="status">
          <p>
            rc-astro: <code>{result.executable}</code>
            {result.cli_version ? ` · ${result.cli_version}` : ''}
          </p>
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
          {result.error && (
            <p className="error-text">
              {result.error}
              {!report && ' The license state each tool reports is unchanged.'}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
