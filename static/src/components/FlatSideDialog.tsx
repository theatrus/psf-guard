import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { windowLabel } from './calibration/flatGroupLabels';
import { useFlatLightChoice } from './calibration/useFlatGroupChoices';
import { useRigClocks } from './calibration/useRigClocks';
import Dialog from './Dialog';
import './FlatSideDialog.css';

interface FlatSideDialogProps {
  dbId: string;
  imageIds: number[];
  onClose: () => void;
}

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * Images' bulk action for flats: put the selected lights before or after
 * each measured change they were shot during, or reset their choices. The
 * changes come from the catalog alone (each light's rig, filter and time),
 * so opening it reads no header and no pixel.
 */
export default function FlatSideDialog({ dbId, imageIds, onClose }: FlatSideDialogProps) {
  const options = useQuery({
    queryKey: ['db', dbId, 'flat-choice-options', [...imageIds].sort((a, b) => a - b).join(',')],
    queryFn: () => apiClient.getFlatLightChoiceOptions(dbId, imageIds),
  });
  const choose = useFlatLightChoice(dbId);
  const clockOf = useRigClocks(dbId);
  const busy = choose.isPending;
  const send = (choice: 'before' | 'after' | 'reset', changeKey?: string) =>
    choose.mutate(
      { image_ids: imageIds, choice, change_key: changeKey },
      { onSuccess: () => void options.refetch() },
    );
  const data = options.data;

  return (
    <Dialog
      open
      title="Flat side of a change"
      className="flat-side-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={
        <>
          <button type="button" className="header-button" disabled={busy || !data || data.with_choices === 0}
            data-testid="flat-side-reset-all" onClick={() => send('reset')}>
            Reset their choices
          </button>
          <button type="button" className="action-button" disabled={busy} onClick={onClose}>Done</button>
        </>
      }
    >
      <p className="flat-side-note">
        {plural(imageIds.length, 'light')} selected. A side you choose wins over its time, its imaging
        session and its pixels, and only these lights change.
      </p>
      {options.isPending && <p className="flat-side-note" role="status">Finding the changes they were shot during…</p>}
      {options.error instanceof Error && (
        <p className="flat-side-error" role="alert">Could not read the changes: {options.error.message}</p>
      )}
      {data && data.changes.length === 0 && (
        <p className="flat-side-note">None of them was shot during a measured change.</p>
      )}
      {data && data.changes.length > 0 && (
        <ul className="flat-side-changes" data-testid="flat-side-changes">
          {data.changes.map((change) => {
            const all = change.lights;
            const chosen = (count: number) => count === all ? 'all' : count > 0 ? `${count}` : null;
            return (
              <li key={change.key} data-testid="flat-side-change">
                <div className="flat-side-what">
                  <strong>{change.what}</strong>
                  <span className="flat-side-note">
                    {windowLabel(change.start, change.end, clockOf(change.rig_uuid))} · {change.camera_side ? `camera side, seen in ${change.filters.join(', ')}` : change.filters.join(', ')}
                  </span>
                  <span className="flat-side-note">
                    {plural(all, 'selected light')} shot during it
                    {chosen(change.chosen_before) ? ` · ${chosen(change.chosen_before)} put before` : ''}
                    {chosen(change.chosen_after) ? ` · ${chosen(change.chosen_after)} put after` : ''}
                  </span>
                </div>
                <span className="flat-side-buttons" role="group" aria-label="Their side of the change">
                  <button type="button" className="header-button" disabled={busy}
                    aria-pressed={change.chosen_before === all} onClick={() => send('before', change.key)}>
                    Before
                  </button>
                  <button type="button" className="header-button" disabled={busy}
                    aria-pressed={change.chosen_after === all} onClick={() => send('after', change.key)}>
                    After
                  </button>
                  <button type="button" className="header-button" disabled={busy || change.chosen_before + change.chosen_after === 0}
                    onClick={() => send('reset', change.key)}>
                    Reset
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {data && data.without_guid > 0 && (
        <p className="flat-side-error">
          {plural(data.without_guid, 'light')} have no GUID to keep a choice under; give them GUIDs in Settings first.
        </p>
      )}
      {choose.error instanceof Error && (
        <p className="flat-side-error" role="alert">Could not set the side: {choose.error.message}</p>
      )}
    </Dialog>
  );
}
