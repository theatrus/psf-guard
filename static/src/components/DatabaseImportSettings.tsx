import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AutoImportSettings, ScanRules } from '../api/types';
import { DEFAULT_AUTOIMPORT_SETTINGS, DEFAULT_SCAN_RULES } from '../api/types';
import type { DbEntry } from '../utils/tauri';
import { useAccess } from '../auth/access';
import UnsavedMark from './UnsavedMark';
import AutoImportFields from './AutoImportFields';
import AutoImportSummary from './AutoImportSummary';
import ScanRulesFields from './ScanRulesFields';

/** Import rules have their own editor, independent of database paths. */
export default function DatabaseImportSettings({ database, canManage, importing, onSaved, onPreview, onFiles }: {
  database: DbEntry;
  canManage: boolean;
  importing: boolean;
  onSaved: () => Promise<void>;
  onPreview: (folders?: string[]) => void;
  onFiles: () => void;
}) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [rulesDraft, setRulesDraft] = useState<ScanRules | null>(null);
  const [autoDraft, setAutoDraft] = useState<AutoImportSettings | null>(null);
  const savedRules = { ...DEFAULT_SCAN_RULES, ...database.scan_rules };
  const savedAuto = { ...DEFAULT_AUTOIMPORT_SETTINGS, ...database.autoimport };
  const rules = rulesDraft ?? savedRules;
  const automatic = autoDraft ?? savedAuto;
  const rulesChanged = JSON.stringify(rules) !== JSON.stringify(savedRules);
  const autoChanged = JSON.stringify(automatic) !== JSON.stringify(savedAuto);
  const dirty = rulesChanged || autoChanged;
  const save = useMutation({
    mutationFn: () => apiClient.updateDatabase(database.id, {
      ...(rulesChanged ? { scan_rules: rules } : {}),
      ...(autoChanged ? { autoimport: automatic } : {}),
    }),
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: ['databases'] });
      await client.invalidateQueries({ queryKey: ['db', database.id] });
      await onSaved();
      setRulesDraft(null);
      setAutoDraft(null);
    },
  });
  const disabled = !canWrite || !canManage || save.isPending;

  return (
    <section className="settings-section" aria-label={`${database.name} import`}>
      <UnsavedMark label={`${database.name} import settings`} dirty={dirty} />
      <h3>Import</h3>
      <p className="review-preferences-note">
        Import frames from this database’s image folders. Preview the results before confirming a manual import.
        {' '}<button type="button" className="link-button" onClick={onFiles}>Edit image folders</button>.
      </p>
      <div className="settings-actions">
        <button
          type="button"
          className="browse-button"
          disabled={!canWrite || importing || dirty || save.isPending}
          onClick={() => onPreview()}
        >
          {importing ? 'Importing…' : 'Preview import'}
        </button>
      </div>
      <AutoImportSummary
        dbId={database.id}
        settings={database.autoimport}
        disabled={!canWrite || dirty || save.isPending || importing}
        onReview={canWrite ? onPreview : undefined}
      />
      <p className="review-preferences-note">
        Scan rules apply to manual and automatic imports from these folders. They do not remove frames already in the database.
        A .psfguardignore file skips its folder and everything below it; its contents are not read.
      </p>
      <fieldset className="database-import-fields" aria-label="Import settings" disabled={disabled}>
        <ScanRulesFields value={rules} onChange={setRulesDraft} />
        <AutoImportFields value={automatic} onChange={setAutoDraft} />
      </fieldset>
      {!canManage && (
        <p className="review-preferences-note">Changing import settings requires database management on this server.</p>
      )}
      {dirty && <p className="review-preferences-note">Save import settings before previewing or running an import.</p>}
      {save.isError && <p className="error-text" role="alert">{save.error.message}</p>}
      <div className="settings-actions">
        <button type="button" className="save-button" disabled={disabled || !dirty} onClick={() => save.mutate()}>
          {save.isPending ? 'Saving…' : 'Save import settings'}
        </button>
      </div>
    </section>
  );
}
