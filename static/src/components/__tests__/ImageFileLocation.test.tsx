import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import ImageFileLocation from '../ImageFileLocation';

const tauriMocks = vi.hoisted(() => ({
  isTauriApp: vi.fn(),
  showImageInFolder: vi.fn(),
}));

vi.mock('../../utils/tauri', () => ({
  isTauriApp: tauriMocks.isTauriApp,
  tauriFileSystem: {
    showImageInFolder: tauriMocks.showImageInFolder,
  },
}));

describe('ImageFileLocation', () => {
  beforeEach(() => {
    tauriMocks.isTauriApp.mockReturnValue(false);
    tauriMocks.showImageInFolder.mockReset();
  });

  it('shows a resolved path without a native action in browser mode', () => {
    render(
      <ImageFileLocation
        dbId="archive"
        filesystemPath="/images/target/frame.fits"
        catalogPath="D:\\Capture\\frame.fits"
      />
    );

    expect(screen.getByTestId('image-file-path')).toHaveTextContent(
      '/images/target/frame.fits'
    );
    expect(screen.getByText('Resolved')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Show in folder' })).not.toBeInTheDocument();
  });

  it('copies the displayed path', async () => {
    const user = userEvent.setup();
    render(
      <ImageFileLocation
        dbId="archive"
        filesystemPath="/images/target/frame.fits"
        catalogPath={null}
      />
    );

    await user.click(screen.getByRole('button', { name: 'Copy path' }));

    expect(screen.getByRole('button', { name: 'Copied' })).toBeInTheDocument();
    expect(await navigator.clipboard.readText()).toBe('/images/target/frame.fits');
  });

  it('falls back to the recorded catalog path when the file is missing', () => {
    tauriMocks.isTauriApp.mockReturnValue(true);

    render(
      <ImageFileLocation
        dbId="archive"
        filesystemPath={null}
        catalogPath={'D:\\Capture\\missing.fits'}
      />
    );

    expect(screen.getByTestId('image-file-path')).toHaveTextContent(
      'D:\\Capture\\missing.fits'
    );
    expect(screen.getByText('Catalog only')).toBeInTheDocument();
    expect(
      screen.getByText('This catalog path does not resolve in the configured image folders.')
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Show in folder' })).not.toBeInTheDocument();
  });

  it('marks an image with no recorded path as unavailable', () => {
    render(
      <ImageFileLocation
        dbId="archive"
        filesystemPath={null}
        catalogPath={null}
      />
    );

    expect(screen.getByText('Unavailable')).toBeInTheDocument();
    expect(screen.getByText('No file path is recorded.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy path' })).not.toBeInTheDocument();
  });

  it('asks Tauri to show a resolved image in its folder', async () => {
    tauriMocks.isTauriApp.mockReturnValue(true);
    tauriMocks.showImageInFolder.mockResolvedValue(undefined);
    const user = userEvent.setup();

    render(
      <ImageFileLocation
        dbId="archive"
        filesystemPath="/images/target/frame.fits"
        catalogPath={null}
      />
    );

    await user.click(screen.getByRole('button', { name: 'Show in folder' }));

    expect(tauriMocks.showImageInFolder).toHaveBeenCalledWith(
      'archive',
      '/images/target/frame.fits'
    );
  });
});
