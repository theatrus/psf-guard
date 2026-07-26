import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { isTauriApp } from '../utils/tauri';
import { checkSignedUpdateVersion, downloadAndInstallSignedUpdate } from '../updates/desktop';
import {
  type AvailableUpdate,
  parseReleaseNotice,
  RELEASES_URL,
  selectNewestUpdate,
} from '../updates/releases';
import { displayVersion } from '../updates/version';

type UpdateInstallState =
  | { phase: 'idle' }
  | { phase: 'checking' }
  | { phase: 'downloading'; percent: number | null }
  | { phase: 'installing' }
  | { phase: 'error'; message: string };

interface UpdateNoticeProps {
  installedVersion?: string;
}

export default function UpdateNotice({ installedVersion }: UpdateNoticeProps) {
  const [currentVersion, setCurrentVersion] = useState(installedVersion ?? '');
  const [signedVersion, setSignedVersion] = useState<string | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);
  const [installState, setInstallState] = useState<UpdateInstallState>({ phase: 'idle' });
  const desktop = isTauriApp();
  const { data: noticeStatus } = useQuery({
    queryKey: ['updateNotice'],
    queryFn: apiClient.getUpdateNotice,
    staleTime: 30_000,
    refetchInterval: (query) => query.state.data?.checking ? 1_000 : 24 * 60 * 60 * 1_000,
  });

  useEffect(() => {
    if (!installedVersion) return;
    void (async () => {
      let version = installedVersion;
      if (desktop) {
        try {
          const { getVersion } = await import('@tauri-apps/api/app');
          version = await getVersion();
        } catch {
          // The server version is the safe fallback for the bundled app.
        }
      }
      setCurrentVersion(version);
      const signed = desktop
        ? await checkSignedUpdateVersion().catch(() => null)
        : null;
      setSignedVersion(signed);
    })().catch(() => {
      // Update checks must never block catalog work.
    });
  }, [desktop, installedVersion]);

  const available: AvailableUpdate | null = useMemo(() => {
    const notice = currentVersion && noticeStatus?.notice
      ? parseReleaseNotice(noticeStatus.notice, currentVersion)
      : null;
    return selectNewestUpdate(notice, signedVersion, currentVersion);
  }, [currentVersion, noticeStatus?.notice, signedVersion]);

  if (!available || dismissedVersion === available.version) return null;

  const signedReady = desktop && signedVersion !== null &&
    displayVersion(signedVersion) === displayVersion(available.version);
  const busy = installState.phase === 'checking' ||
    installState.phase === 'downloading' || installState.phase === 'installing';
  const status = installState.phase === 'checking'
    ? 'Checking signed package…'
    : installState.phase === 'downloading'
      ? installState.percent === null
        ? 'Downloading update…'
        : `Downloading ${installState.percent}%…`
      : installState.phase === 'installing'
        ? 'Installing and restarting…'
        : installState.phase === 'error'
          ? installState.message
          : available.urgency === 'required'
            ? `Installed ${displayVersion(currentVersion)} is no longer supported.`
            : `Installed ${displayVersion(currentVersion)}`;

  const install = async () => {
    setInstallState({ phase: 'checking' });
    try {
      const version = await downloadAndInstallSignedUpdate((progress) => {
        setInstallState(progress.phase === 'installing'
          ? { phase: 'installing' }
          : { phase: 'downloading', percent: progress.percent });
      });
      if (!version) throw new Error('The signed update is no longer available.');
    } catch {
      setInstallState({
        phase: 'error',
        message: 'Install failed. You can still download the release.',
      });
    }
  };

  return (
    <aside className={`update-notice update-notice--${available.urgency}`} role="status">
      <span className="update-notice__copy">
        <strong>{displayVersion(available.version)} available</strong>
        <span>{available.summary ?? status}</span>
        {available.summary && <small>{status}</small>}
      </span>
      {signedReady ? (
        <>
          <button type="button" disabled={busy} onClick={() => void install()}>
            {busy ? 'Working…' : 'Install update'}
          </button>
          <a href={available.url || RELEASES_URL} target="_blank" rel="noreferrer">Notes</a>
        </>
      ) : (
        <a href={available.url || RELEASES_URL} target="_blank" rel="noreferrer">
          Release notes
        </a>
      )}
      {!busy && (
        <button
          type="button"
          aria-label={`Dismiss ${displayVersion(available.version)} update notice`}
          onClick={() => setDismissedVersion(available.version)}
        >
          Later
        </button>
      )}
    </aside>
  );
}
