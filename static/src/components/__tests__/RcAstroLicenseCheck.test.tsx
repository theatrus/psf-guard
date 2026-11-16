import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import RcAstroLicenseCheck from '../RcAstroLicenseCheck';
import type { RcAstroLicenseCheck as Check, RcAstroLicenseCheckStatus as Status } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 }, mutations: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const running: Status = { key: 'server', computer: 'this server', running: true, started_at: 1 };
const ended = (check?: Check, problem?: string): Status =>
  ({ ...running, running: false, check, problem });

const registered: Check = {
  executable: '/opt/rc-astro/rc-astro',
  cli_version: '2.6.10',
  report: {
    computer: 'ann@studio',
    account_email: 'user@example.com',
    activation_url: 'https://www.rc-astro.com/account',
    products: [
      { product: 'bxt', name: 'BlurXTerminator', status: 'permanent', message: 'Permanent license through ML5' },
      { product: 'nxt', name: 'NoiseXTerminator', status: 'limit', message: 'Not activated - activate: rc-astro license --activate nxt', used: 4, limit: 4 },
      { product: 'zxt', name: 'ZoomXTerminator', status: 'leased', message: 'Leased until Friday' },
    ],
  },
};

/** The component over a check that ends with `result` at once. */
function mount(where: string, result: Status) {
  const start = vi.fn(async () => result);
  const read = vi.fn(async () => result);
  const stop = vi.fn(async () => {});
  render(<RcAstroLicenseCheck where={where} start={start} read={read} stop={stop} />, { wrapper: wrapper() });
  return { start, read, stop };
}

describe('RcAstroLicenseCheck', () => {
  it('starts a check only when asked, polls it, and shows what RC-Astro wrote', async () => {
    const start = vi.fn(async () => running);
    let polls = 0;
    const read = vi.fn(async () => {
      polls += 1;
      return polls < 2 ? running : ended(registered);
    });
    const stop = vi.fn(async () => {});
    render(<RcAstroLicenseCheck where="this server" start={start} read={read} stop={stop} />, { wrapper: wrapper() });

    const button = screen.getByRole('button', { name: 'Check license on this server' });
    expect(button).toHaveTextContent('Check license');
    expect(start).not.toHaveBeenCalled();
    expect(screen.queryByRole('status')).toBeNull();

    await userEvent.click(button);
    // While the job runs the button waits and the check can be stopped.
    expect(await screen.findByRole('button', { name: 'Stop the license check on this server' })).toBeInTheDocument();
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent('Checking…');

    const report = await screen.findByRole('status', {}, { timeout: 4000 });
    expect(start).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledWith('server');
    expect(report).toHaveTextContent('rc-astro: /opt/rc-astro/rc-astro · 2.6.10');
    expect(report).toHaveTextContent('Computer: ann@studio');
    expect(report).toHaveTextContent('RC-Astro account: user@example.com');
    expect(report).toHaveTextContent('BlurXTerminator: Permanent license through ML5');
    expect(report).toHaveTextContent('NoiseXTerminator: Not activated - activate: rc-astro license --activate nxt');
    // A status this build has never heard of still shows its message.
    expect(report).toHaveTextContent('ZoomXTerminator: Leased until Friday');
    expect(report).not.toHaveTextContent('is unchanged');
    expect(screen.queryByRole('link', { name: 'Get an activation code' })).toBeNull();
    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.queryByRole('button', { name: /Stop the license check/ })).toBeNull();
  });

  it('stops a check under way', async () => {
    const stop = vi.fn(async () => {});
    render(<RcAstroLicenseCheck where="studio-pc" start={async () => ({ ...running, key: 'worker-1', computer: 'studio-pc' })}
      read={async () => ({ ...running, key: 'worker-1', computer: 'studio-pc' })} stop={stop} />, { wrapper: wrapper() });
    await userEvent.click(screen.getByRole('button', { name: 'Check license on studio-pc' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Stop the license check on studio-pc' }));
    expect(stop).toHaveBeenCalledWith('worker-1');
  });

  it('says a computer has no account, where to get a code, and what to run in a terminal', async () => {
    mount('studio-pc', ended({
      executable: 'C:\\Program Files\\RC-Astro\\CLI\\rc-astro.exe',
      report: {
        computer: 'svc@worker',
        activation_url: 'https://www.rc-astro.com/account',
        products: [{ product: 'bxt', name: 'BlurXTerminator', status: 'required', message: 'Not activated' }],
        attention: {
          reason: 'unregistered',
          message: 'This computer is not registered to an RC-Astro account',
          command: 'rc-astro license <code>',
        },
      },
      error: 'No license',
    }));
    await userEvent.click(screen.getByRole('button', { name: 'Check license on studio-pc' }));

    const report = await screen.findByRole('status');
    expect(report).toHaveTextContent('Computer: svc@worker');
    expect(report).toHaveTextContent('This computer is not registered to an RC-Astro account.');
    expect(screen.getByRole('link', { name: 'Get an activation code' }))
      .toHaveAttribute('href', 'https://www.rc-astro.com/account');
    expect(report).toHaveTextContent('Run in a terminal on that computer: rc-astro license <code>');
    // A refusal's error shows beside its report, as RC-Astro's words.
    expect(report).toHaveTextContent('RC-Astro reported: No license');
    expect(report).not.toHaveTextContent('is unchanged');
  });

  it('marks RC-Astro\u2019s error as its own when the check is offline, and says the local state stays', async () => {
    mount('this server', ended({
      executable: '/opt/rc-astro/rc-astro',
      error: 'Could not reach the RC-Astro license server',
    }));
    await userEvent.click(screen.getByRole('button', { name: 'Check license on this server' }));

    const report = await screen.findByRole('status');
    expect(report).toHaveTextContent('RC-Astro reported: Could not reach the RC-Astro license server');
    expect(report).toHaveTextContent('The license state each tool reports is unchanged.');
    expect(report).not.toHaveTextContent('Computer:');
  });

  it('says PSF Guard\u2019s own failure in its own words, once', async () => {
    mount('this server', ended({
      executable: '/opt/rc-astro/rc-astro',
      failure: 'rc-astro ended (exit status: 127) without a license report. It printed: libtorch.so: cannot open',
    }));
    await userEvent.click(screen.getByRole('button', { name: 'Check license on this server' }));

    const report = await screen.findByRole('status');
    expect(report).toHaveTextContent('rc-astro ended (exit status: 127) without a license report.');
    expect(report).not.toHaveTextContent('RC-Astro reported');
    expect(report.textContent?.match(/is unchanged/g)).toHaveLength(1);
  });

  it('shows why a worker gave no check, and why one could not be started at all', async () => {
    mount('studio-pc', ended(undefined,
      'studio-pc did not answer within 90 seconds. Check that the worker is connected and try again, or run `psf-guard-cli rc-astro license` on that computer.'));
    await userEvent.click(screen.getByRole('button', { name: 'Check license on studio-pc' }));
    expect(await screen.findByRole('status')).toHaveTextContent('studio-pc did not answer within 90 seconds.');

    const refused = async (): Promise<Status> => {
      throw new Error('attic-pc cannot be asked for a license check');
    };
    render(<RcAstroLicenseCheck where="attic-pc" start={refused} read={refused} stop={async () => {}} />, { wrapper: wrapper() });
    await userEvent.click(screen.getByRole('button', { name: 'Check license on attic-pc' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('attic-pc cannot be asked for a license check');
  });
});
