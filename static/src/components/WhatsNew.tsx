import { useState } from 'react';
import Dialog from './Dialog';
import {
  acknowledgeVersion, normalizedVersion, notesForVersion, readAcknowledgedVersion,
  releaseHighlights, releaseNotesUrl, type ReleaseHighlights,
} from '../updates/whatsNew';
import { displayVersion, isVersionNewer } from '../updates/version';
import './WhatsNew.css';

interface WhatsNewProps {
  version?: string;
  requested: boolean;
  blocked?: boolean;
  onClose: () => void;
}

const sections = [
  ['deprecations', 'Deprecations'],
  ['upgradeNotes', 'Upgrade notes'],
  ['highlights', 'New features'],
] as const;

// App keys this component by server version so a running tab notices upgrades.
export default function WhatsNew({ version, requested, blocked = false, onClose }: WhatsNewProps) {
  const current = normalizedVersion(version);
  const [acknowledged, setAcknowledged] = useState(readAcknowledgedVersion);
  const [dismissed, setDismissed] = useState(false);
  const [dontShowAgain, setDontShowAgain] = useState(false);
  const bundled = releaseHighlights.some(release => release.version === current);
  const pending = !!current && bundled && (!acknowledged || isVersionNewer(current, acknowledged));
  const open = !blocked && (requested || (pending && !dismissed));
  // Include skipped releases on upgrade. A first visit shows the current release;
  // the menu also gives access to older bundled notes.
  const notes = requested
    ? bundled ? notesForVersion(current) : []
    : acknowledged
      ? notesForVersion(current, acknowledged)
      : releaseHighlights.filter(release => release.version === current);

  const close = () => {
    if (dontShowAgain && current && pending) {
      acknowledgeVersion(current);
      setAcknowledged(current);
    }
    setDismissed(true);
    setDontShowAgain(false);
    onClose();
  };

  return (
    <Dialog
      open={open}
      title={current ? `What's new in PSF Guard ${displayVersion(current)}` : "What's new in PSF Guard"}
      onClose={close}
      className="whats-new"
      footer={<>
        {pending && <label className="whats-new-preference">
          <input type="checkbox" checked={dontShowAgain} onChange={event => setDontShowAgain(event.target.checked)} />
          Don't show again for this version
        </label>}
        <button type="button" className="action-button" onClick={close}>Done</button>
      </>}
    >
      <p className="dialog-intro">Read these notes again from Menu › What's new.</p>
      {!bundled && <p>No release highlights are available for this version.</p>}
      {sections.map(([field, title]) => <NotesSection key={field} field={field} title={title} notes={notes} />)}
      {notes.length > 0 && <nav className="whats-new-links" aria-label="Full release notes">
        {notes.map(release => <a key={release.version} href={releaseNotesUrl(release.version)} target="_blank" rel="noreferrer">
          Full release notes for {displayVersion(release.version)}
        </a>)}
      </nav>}
      {!bundled && current && <a href={releaseNotesUrl(current)} target="_blank" rel="noreferrer">Full release notes for {displayVersion(current)}</a>}
    </Dialog>
  );
}

function NotesSection({ field, title, notes }: {
  field: 'deprecations' | 'upgradeNotes' | 'highlights';
  title: string;
  notes: ReleaseHighlights[];
}) {
  const releases = notes.filter(release => release[field].length > 0);
  if (!releases.length) return null;
  return <section className={`whats-new-section whats-new-${field}`}>
    <h3>{title}</h3>
    {releases.map(release => <div key={release.version}>
      {notes.length > 1 && <h4>{displayVersion(release.version)}</h4>}
      <ul>{release[field].map(item => <li key={item.title}>
        <strong>{item.title}</strong><p>{item.body}</p>
      </li>)}</ul>
    </div>)}
  </section>;
}
