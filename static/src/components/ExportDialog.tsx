import { useState } from 'react';
import { apiClient } from '../api/client';
import type { ExportChoice, ExportLayout, ExportPlacement } from '../api/types';
import Dialog from './Dialog';

/** One export the user has asked for, awaiting its layout choice. */
export interface ExportRequest {
  dbId: string;
  scope: { project_id?: number; target_id?: number };
  label: string;
  /**
   * Which affordance applies: `local` picks a folder through the native
   * picker (desktop), `server` runs in the server's export directory,
   * `download` streams a zip.
   */
  kind: 'local' | 'server' | 'download';
}

interface ExportDialogProps {
  request: ExportRequest;
  /** What the layout choice starts from, per the settings panel. */
  defaultLayout: ExportLayout;
  /**
   * The folder the database's image directories share, as the server sees
   * it: what a referenced export names frames below unless told otherwise.
   */
  sourceRoot?: string;
  busy: boolean;
  onClose: () => void;
  /** Runs the local or server export with the chosen options. */
  onConfirm: (choice: ExportChoice) => void;
}

const LAYOUT_HELP: Record<ExportLayout, string> = {
  standard:
    'Grouped by target: <target>/LIGHT/<filter>, with BIAS, DARK and DARKFLAT at the root.',
  wbpp: 'One folder per frame type, with dark flats among the darks, ready for WeightedBatchPreprocessing. Includes run-wbpp scripts.',
};

interface PlacementOption {
  value: ExportPlacement;
  label: string;
  help: string;
}

/** What the server export offers: it clones where it can and copies elsewhere. */
const SERVER_PLACEMENTS: PlacementOption[] = [
  {
    value: 'reflink',
    label: 'Copy',
    help: 'A clone where the filesystem supports it, a copy elsewhere. The export stands on its own.',
  },
  {
    value: 'symlink',
    label: 'Link to the originals',
    help: 'Symbolic links, so the tree costs no space and can point at a network mount. Whatever reads it must see the originals at the same paths.',
  },
  {
    value: 'reference',
    label: 'Reference in place',
    help: 'Copies nothing. The run-wbpp scripts name every frame where it already is. WBPP then pools each filter\u2019s flats across nights; needs the WBPP layout.',
  },
];

/** What the desktop export offers: it hardlinks on the same drive by default. */
const LOCAL_PLACEMENTS: PlacementOption[] = [
  {
    value: 'hardlink',
    label: 'Copy',
    help: 'A hard link on the same drive, a copy elsewhere. The export stands on its own.',
  },
  SERVER_PLACEMENTS[1],
  SERVER_PLACEMENTS[2],
];

/**
 * The choices made at export time. Every export affordance funnels through
 * here so the choice is per export, not a page-wide mode.
 */
export default function ExportDialog({
  request,
  defaultLayout,
  sourceRoot,
  busy,
  onClose,
  onConfirm,
}: ExportDialogProps) {
  const [layout, setLayout] = useState<ExportLayout>(defaultLayout);
  // Ungraded lights are what a fresh night mostly is, and the stack previews
  // include them, so an export does too unless told otherwise.
  const [includePending, setIncludePending] = useState(true);
  const placements =
    request.kind === 'local' ? LOCAL_PLACEMENTS : request.kind === 'server' ? SERVER_PLACEMENTS : [];
  const [placement, setPlacement] = useState<ExportPlacement>(
    placements[0]?.value ?? 'copy'
  );
  const [localRoot, setLocalRoot] = useState(sourceRoot ?? '');
  const [remoteRoot, setRemoteRoot] = useState('');
  // Referencing frames in place only produces the runner, which the standard
  // layout has none of.
  const referenceUnavailable = layout !== 'wbpp';
  const effectivePlacement =
    placement === 'reference' && referenceUnavailable ? placements[0].value : placement;

  const confirmLabel =
    request.kind === 'local'
      ? 'Choose folder…'
      : request.kind === 'server'
        ? 'Start export'
        : 'Download zip';

  const choice = (): ExportChoice => ({
    layout,
    include_pending: includePending,
    ...(request.kind === 'download'
      ? {}
      : {
          placement: effectivePlacement,
          ...(effectivePlacement === 'reference' && localRoot.trim()
            ? { local_root: localRoot.trim() }
            : {}),
          ...(effectivePlacement === 'reference' && remoteRoot.trim()
            ? { remote_root: remoteRoot.trim() }
            : {}),
        }),
  });

  return (
    <Dialog
      open
      title={`Export ${request.label}`}
      onClose={onClose}
      className="export-dialog"
      footer={
        <>
          <button type="button" className="header-button" onClick={onClose}>
            Cancel
          </button>
          {request.kind === 'download' ? (
            <a
              className="action-button export-dialog-download"
              href={apiClient.exportDownloadUrl(request.dbId, {
                ...request.scope,
                layout,
                include_pending: includePending,
              })}
              onClick={onClose}
            >
              {confirmLabel}
            </a>
          ) : (
            <button
              type="button"
              className="action-button"
              disabled={busy}
              onClick={() => onConfirm(choice())}
            >
              {confirmLabel}
            </button>
          )}
        </>
      }
    >
      <p className="export-dialog-scope">
        {includePending ? 'Accepted and ungraded' : 'Accepted'} lights and their calibration
        frames; rejects excluded. Each night&apos;s flats keep their own folder beside the lights
        they calibrate.
      </p>
      <label className="export-dialog-option">
        <input
          type="checkbox"
          checked={includePending}
          onChange={(event) => setIncludePending(event.target.checked)}
        />
        <span>
          <strong>Include ungraded lights</strong>
          <small>Frames the grader has not judged yet, as the stack previews do.</small>
        </span>
      </label>
      <fieldset className="export-layout-options">
        <legend>Layout</legend>
        {(['standard', 'wbpp'] as const).map((option) => (
          <label key={option} className="export-layout-option">
            <input
              type="radio"
              name="export-layout"
              value={option}
              checked={layout === option}
              onChange={() => setLayout(option)}
            />
            <span>
              <strong>{option === 'wbpp' ? 'WBPP' : 'Grouped by target'}</strong>
              <small>{LAYOUT_HELP[option]}</small>
            </span>
          </label>
        ))}
      </fieldset>
      {placements.length > 0 && (
        <fieldset className="export-layout-options">
          <legend>Files</legend>
          {placements.map((option) => {
            const unavailable = option.value === 'reference' && referenceUnavailable;
            return (
              <label key={option.value} className="export-layout-option">
                <input
                  type="radio"
                  name="export-placement"
                  value={option.value}
                  checked={effectivePlacement === option.value}
                  disabled={unavailable}
                  onChange={() => setPlacement(option.value)}
                />
                <span>
                  <strong>{option.label}</strong>
                  <small>{option.help}</small>
                </span>
              </label>
            );
          })}
          {effectivePlacement === 'reference' && (
            <>
              <label className="export-dialog-remote-root">
                <span>
                  Image folder on this server
                  <small>
                    The scripts name every frame below this folder. Frames outside it keep
                    their full path.
                  </small>
                </span>
                <input
                  type="text"
                  value={localRoot}
                  placeholder="/mnt/nas/astro"
                  onChange={(event) => setLocalRoot(event.target.value)}
                />
              </label>
              <label className="export-dialog-remote-root">
                <span>
                  The same folder as PixInsight sees it
                  <small>
                    Leave empty when PixInsight runs on this server. Otherwise the drive
                    letter or share that folder appears as on that machine.
                  </small>
                </span>
                <input
                  type="text"
                  value={remoteRoot}
                  placeholder="P:\\ or \\\\nas\\astro or /Volumes/astro"
                  onChange={(event) => setRemoteRoot(event.target.value)}
                />
              </label>
            </>
          )}
        </fieldset>
      )}
    </Dialog>
  );
}
