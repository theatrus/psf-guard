import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import RcAstroLicenseCheck from '../RcAstroLicenseCheck';
import type { RcAstroLicenseCheck as Check } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

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

describe('RcAstroLicenseCheck', () => {
  it('checks only when asked, then shows the computer, the account and each product as RC-Astro wrote it', async () => {
    const check = vi.fn(async () => registered);
    render(<RcAstroLicenseCheck where="this server" check={check} />, { wrapper: wrapper() });

    const button = screen.getByRole('button', { name: 'Check license on this server' });
    expect(button).toHaveTextContent('Check license');
    expect(check).not.toHaveBeenCalled();
    expect(screen.queryByRole('status')).toBeNull();

    await userEvent.click(button);
    const report = await screen.findByRole('status');
    expect(check).toHaveBeenCalledTimes(1);
    expect(report).toHaveTextContent('rc-astro: /opt/rc-astro/rc-astro · 2.6.10');
    expect(report).toHaveTextContent('Computer: ann@studio');
    expect(report).toHaveTextContent('RC-Astro account: user@example.com');
    expect(report).toHaveTextContent('BlurXTerminator: Permanent license through ML5');
    expect(report).toHaveTextContent('NoiseXTerminator: Not activated - activate: rc-astro license --activate nxt');
    // A status this build has never heard of still shows its message.
    expect(report).toHaveTextContent('ZoomXTerminator: Leased until Friday');
    expect(screen.queryByRole('link', { name: 'Get an activation code' })).toBeNull();
  });

  it('says a computer has no account, where to get a code, and what to run in a terminal', async () => {
    const check = async (): Promise<Check> => ({
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
    });
    render(<RcAstroLicenseCheck where="studio-pc" check={check} />, { wrapper: wrapper() });
    await userEvent.click(screen.getByRole('button', { name: 'Check license on studio-pc' }));

    const report = await screen.findByRole('status');
    expect(report).toHaveTextContent('Computer: svc@worker');
    expect(report).toHaveTextContent('This computer is not registered to an RC-Astro account.');
    expect(screen.getByRole('link', { name: 'Get an activation code' }))
      .toHaveAttribute('href', 'https://www.rc-astro.com/account');
    expect(report).toHaveTextContent('Run in a terminal on that computer: rc-astro license <code>');
    // A refusal's error shows beside its report.
    expect(report).toHaveTextContent('No license');
    expect(report).not.toHaveTextContent('is unchanged');
  });

  it('shows RC-Astro\u2019s error when the check is offline, and says the local state stays', async () => {
    const check = async (): Promise<Check> => ({
      executable: '/opt/rc-astro/rc-astro',
      error: 'Could not reach the RC-Astro license server',
    });
    render(<RcAstroLicenseCheck where="this server" check={check} />, { wrapper: wrapper() });
    await userEvent.click(screen.getByRole('button', { name: 'Check license on this server' }));

    const report = await screen.findByRole('status');
    expect(report).toHaveTextContent(
      'Could not reach the RC-Astro license server The license state each tool reports is unchanged.'
    );
    expect(report).not.toHaveTextContent('Computer:');
  });

  it('shows why a check could not be made at all', async () => {
    const check = async (): Promise<Check> => {
      throw new Error('studio-pc cannot be asked for a license check');
    };
    render(<RcAstroLicenseCheck where="studio-pc" check={check} />, { wrapper: wrapper() });
    await userEvent.click(screen.getByRole('button', { name: 'Check license on studio-pc' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('studio-pc cannot be asked for a license check');
  });
});
