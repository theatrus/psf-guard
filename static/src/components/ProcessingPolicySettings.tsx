import { useState, type ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { DatabasePolicy, PolicyOverrides, PolicyValues, ProcessingPolicies } from '../api/processingPolicies';
import { useAccess } from '../auth/access';
import UnsavedMark from './UnsavedMark';
import QualityBackfillControls from './QualityBackfillControls';
import SettingsOverrideItem from './SettingsOverrideItem';
import SettingsRuleOverride from './SettingsRuleOverride';
import SettingsToggle from './SettingsToggle';

const FIELDS = {
  analyze_new_frames: {
    label: 'Analyze new frames as they arrive',
    help: 'Analyze frames received by sync, uploads or automatic import. Turning this on also queues frames already waiting for analysis.',
  },
  scan_calibrated_copies: {
    label: 'Measure quality on the calibrated copy',
    help: 'Use a light’s calibrated copy when available. Registered copies are never measured. Changes apply the next time quality analysis runs.',
  },
  pair_calibrated_copies: {
    label: 'Pair calibrated and registered copies with their lights',
    help: 'Record processed copies with their original light instead of importing them as separate frames. Changes apply to future imports and pairing scans.',
  },
};
type Key = keyof PolicyValues;

export default function ProcessingPolicySettings({ kind, canManage }: {
  kind: 'quality' | 'pairing';
  canManage: boolean;
}) {
  const query = useQuery({
    queryKey: ['processing-policies'],
    queryFn: apiClient.getProcessingPolicies,
    retry: false,
  });
  const [open, setOpen] = useState<string | null>(null);
  const keys: Key[] = kind === 'quality'
    ? ['analyze_new_frames', 'scan_calibrated_copies']
    : ['pair_calibrated_copies'];
  const title = kind === 'quality' ? 'Quality analysis' : 'Calibrated copies';

  if (query.isPending) {
    return (
      <section className="settings-section">
        <h3>{title}</h3>
        <p role="status">Loading processing settings…</p>
      </section>
    );
  }
  if (query.isError) {
    return (
      <section className="settings-section">
        <h3>{title}</h3>
        <p className="error-text" role="alert">Could not load processing settings. {query.error.message}</p>
        <button type="button" onClick={() => void query.refetch()}>Retry</button>
      </section>
    );
  }

  return (
    <section className="settings-section" aria-label={title}>
      <h3>{title}</h3>
      <p className="review-preferences-note">
        Global settings apply to databases without an override. Existing database choices are kept as overrides.
      </p>
      <PolicyEditor keys={keys} title={title} data={query.data} canManage={canManage} />
      <h4>Databases</h4>
      <p className="review-preferences-note">
        Database overrides replace the global settings. All other fields use the global values.
      </p>
      {query.data.databases.length === 0 ? (
        <p className="calibration-group-note">Add a database to configure overrides.</p>
      ) : (
        <ul className="calibration-rig-list">
          {query.data.databases.map(entry => {
            const expanded = open === entry.id;
            const editorId = `processing-${kind}-${entry.id}`;
            return (
              <SettingsOverrideItem
                key={entry.id}
                name={entry.name}
                count={keys.filter(key => entry.overrides[key] !== null).length}
                expanded={expanded}
                editorId={editorId}
                onToggle={() => setOpen(expanded ? null : entry.id)}
              >
                <div id={editorId} hidden={!expanded}>
                  <PolicyEditor keys={keys} title={title} data={query.data} database={entry} canManage={canManage}>
                    {kind === 'quality' && (
                      <QualityBackfillControls dbId={entry.id} canManage={canManage} showArrivalSetting={false} />
                    )}
                  </PolicyEditor>
                </div>
              </SettingsOverrideItem>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function PolicyEditor({ keys, title, data, database, canManage, children }: {
  keys: Key[];
  title: string;
  data: ProcessingPolicies;
  database?: DatabasePolicy;
  canManage: boolean;
  children?: ReactNode;
}) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [edits, setEdits] = useState<Partial<PolicyOverrides>>({});
  const saved = database?.overrides ?? data.defaults;
  const dirty = keys.some(key => key in edits && edits[key] !== saved[key]);
  const label = database ? `${database.name} processing` : 'processing defaults';
  const save = useMutation({
    mutationFn: () => apiClient.updateProcessingPolicies({
      database_id: database?.id,
      values: Object.fromEntries(keys
        .filter(key => key in edits && edits[key] !== saved[key])
        .map(key => [key, edits[key]])),
    }),
    onSuccess: updated => {
      client.setQueryData(['processing-policies'], updated);
      for (const entry of updated.databases) {
        void client.invalidateQueries({ queryKey: ['db', entry.id, 'quality-backfill'] });
        void client.invalidateQueries({ queryKey: ['db', entry.id, 'calibrated-copies'] });
      }
      setEdits({});
    },
  });
  const disabled = !canWrite || !canManage || save.isPending;

  return (
    <div
      className={database ? 'calibration-rig-editor' : undefined}
      role="region"
      aria-label={database ? `${database.name} overrides` : 'Global settings'}
    >
      <UnsavedMark label={label} dirty={dirty} />
      <fieldset className="calibration-settings-group calibration-matching-group" disabled={disabled}>
        <legend>{database ? title : 'Global settings'}</legend>
        {keys.map(key => {
          const own = key in edits ? edits[key] : saved[key];
          const value = own ?? data.defaults[key];
          const control = (
            <SettingsToggle
              label={FIELDS[key].label}
              value={value}
              onChange={next => setEdits(previous => ({ ...previous, [key]: next }))}
            />
          );
          return (
            <div key={key} className="review-preference calibration-rule-row">
              <span>{FIELDS[key].label}<small>{FIELDS[key].help}</small></span>
              <div className="calibration-rule-setting">
                {database ? (
                  <SettingsRuleOverride
                    label={FIELDS[key].label}
                    isOwn={own !== null}
                    inheritedValue={data.defaults[key] ? 'On' : 'Off'}
                    source="global"
                    target="database"
                    onToggle={() => setEdits(previous => ({
                      ...previous,
                      [key]: own === null ? data.defaults[key] : null,
                    }))}
                  >
                    {control}
                  </SettingsRuleOverride>
                ) : control}
              </div>
            </div>
          );
        })}
      </fieldset>
      {!canManage && (
        <p className="review-preferences-note">Changing processing settings requires database management on this server.</p>
      )}
      {save.isError && <p className="error-text" role="alert">{save.error.message}</p>}
      <div className="settings-actions">
        <button type="button" className="save-button" disabled={disabled || !dirty} onClick={() => save.mutate()}>
          {save.isPending ? 'Saving…' : database ? 'Save database' : 'Save global settings'}
        </button>
        {database && (
          <button
            type="button"
            className="calibration-rig-reset"
            disabled={disabled || keys.every(key => (key in edits ? edits[key] : saved[key]) === null)}
            onClick={() => setEdits(Object.fromEntries(keys.map(key => [key, null])))}
          >
            Use global settings for all fields
          </button>
        )}
      </div>
      {children && (
        <fieldset className="calibration-settings-group">
          <legend>Run quality analysis</legend>
          {children}
        </fieldset>
      )}
    </div>
  );
}
