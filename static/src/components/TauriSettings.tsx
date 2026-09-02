import {
  useCallback,
  useEffect,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { isTauriApp, tauriConfig, tauriFileSystem } from '../utils/tauri';
import type {
  ImportFolder,
  ImportScope,
  RemoteImageUploadPlacement,
} from '../api/types';
import type { DbEntry, DbRegistry } from '../utils/tauri';
import { apiClient } from '../api/client';
import { useAccess } from '../auth/access';
import type { SettingsIntent } from '../utils/settingsIntent';
import ReviewPreferences from './ReviewPreferences';
import CalibrationMatchingSettings from './CalibrationMatchingSettings';
import ExportDefaultsSettings from './ExportDefaultsSettings';
import type { DatabaseSummary } from '../api/types';
import { describeImportProgress, useImportJob } from '../hooks/useImportJob';
import { starMetadataFillEnabled } from '../hooks/useStarMetadataFill';
import QualityBackfillControls from './QualityBackfillControls';
import RemotePeerSync from './RemotePeerSync';
import SchedulerSyncControls from './SchedulerSyncControls';
import RemoteSyncPreviews from './RemoteSyncPreviews';
import SeizaCatalogControls from './SeizaCatalogControls';
import ProcessingSetupsManager from './ProcessingSetupsManager';
import CalibrationLibrarySummary from './CalibrationLibrarySummary';
import UserManagement from './UserManagement';
import './TauriSettings.css';

/**
 * Settings groups unrelated jobs into named tabs so each stays easy to find.
 */
type SettingsTab = 'databases' | 'catalogs' | 'sync' | 'setups' | 'review' | 'users';

const DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE =
  '%YEAR%/%TARGET%/%NIGHT%/%TYPE%';
const REMOTE_UPLOAD_DIRECTORY_TEMPLATE_PRESETS = [
  '%YEAR%/%TARGET%/%NIGHT%/%TYPE%',
  '%TARGET%/%NIGHT%/%TYPE%',
  '%TARGET%/%DATE%/%TYPE%',
  '%TARGET%/%TYPE%/%FILTER%',
  '%NIGHT%/%TARGET%/%TYPE%',
] as const;

const isRemoteUploadDirectoryTemplatePreset = (value: string) =>
  REMOTE_UPLOAD_DIRECTORY_TEMPLATE_PRESETS.some((preset) => preset === value);

interface TauriSettingsProps {
  isOpen: boolean;
  onClose: () => void;
  /**
   * Form to land on when the modal opens. Lets the overview's empty state
   * send the user straight to "add an existing database" or "build one from
   * image folders" instead of dropping them at the top of the modal.
   */
  initialIntent?: SettingsIntent | null;
}

/**
 * Multi-database settings modal.
 *
 * Lists every configured database and lets the user edit name / image_dirs in
 * place, remove a database, or add a new one. Slug renaming is intentionally
 * not exposed here (breaks every existing bookmark for that DB) — users who
 * really want to rename a slug can hand-edit `config.json`.
 *
 * Works in both Tauri and browser/CLI-server mode:
 * - Tauri mode prefers the in-process commands so add/edit feel native and
 *   file pickers open OS dialogs.
 * - Browser mode falls back to HTTP `POST/PUT/DELETE /api/databases` so the
 *   same UI is usable when the server was launched via `psf-guard server`.
 *   The file pickers degrade to plain text inputs.
 */
export default function TauriSettings({
  isOpen,
  onClose,
  initialIntent = null,
}: TauriSettingsProps) {
  const isTauri = isTauriApp();
  const access = useAccess();
  const queryClient = useQueryClient();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  // CRUD requires either Tauri (in-process commands always allowed) or the
  // CLI server having been launched with --allow-database-management. The
  // gate is enforced server-side; we mirror it here to hide UI that would
  // just 403.
  const managementAllowed = isTauri || (serverInfo?.allow_database_management ?? false);
  const [registry, setRegistry] = useState<DbRegistry | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isApplying, setIsApplying] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string>('');

  // Inline edit/add form state.
  const [editingId, setEditingId] = useState<string | null>(null); // null = add, slug = edit
  const [formName, setFormName] = useState('');
  const [formDbPath, setFormDbPath] = useState('');
  const [formImageDirs, setFormImageDirs] = useState<string[]>([]);
  const [formRemoteUploadEnabled, setFormRemoteUploadEnabled] = useState(false);
  const [formRemoteSyncEnabled, setFormRemoteSyncEnabled] = useState(false);
  const [formRemoteUploadDir, setFormRemoteUploadDir] = useState('');
  const [formRemoteUploadPlacement, setFormRemoteUploadPlacement] =
    useState<RemoteImageUploadPlacement>('flat');
  /**
   * Whether uploads follow the layout detected from the catalog ('catalog')
   * or the preset/custom template below ('preset'). Choosing a template is a
   * decision: it replaces the catalog match rather than hiding behind it as
   * the fallback (#399).
   */
  const [formRemoteUploadLayoutChoice, setFormRemoteUploadLayoutChoice] =
    useState<'catalog' | 'preset'>('preset');
  const [formRemoteUploadDirectoryTemplate, setFormRemoteUploadDirectoryTemplate] =
    useState(DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE);
  const [formRemoteUploadCatalogMatch, setFormRemoteUploadCatalogMatch] =
    useState<{ template: string; samples: number } | null>(null);
  const [formRemoteUploadRescanRequested, setFormRemoteUploadRescanRequested] =
    useState(false);
  const [formExportDir, setFormExportDir] = useState('');
  const [formRemoteUploadToken, setFormRemoteUploadToken] = useState('');
  const [formRemoteUploadTokenConfigured, setFormRemoteUploadTokenConfigured] =
    useState(false);
  const [formRemoteUploadTokenRevealed, setFormRemoteUploadTokenRevealed] =
    useState(false);
  const [formRemoteUploadTokenCopyState, setFormRemoteUploadTokenCopyState] =
    useState<'idle' | 'copied' | 'failed'>('idle');
  const [showAddForm, setShowAddForm] = useState(false);
  const [activeTab, setActiveTab] = useState<SettingsTab>('databases');
  // true = "create a brand-new TS database from image folders" flow (no
  // existing .sqlite required; the server bootstraps the full TS schema).
  const [createMode, setCreateMode] = useState(false);
  const [createAnalyzeQuality, setCreateAnalyzeQuality] = useState(false);
  const [importAnalyzeQuality, setImportAnalyzeQuality] = useState(false);

  // Slug of the database whose import job we're currently tracking; drives
  // the 1s progress poll + the progress panel at the bottom of the modal.
  const [importDbId, setImportDbId] = useState<string | null>(null);
  const { progress: importProgress, isRunning: importRunning } = useImportJob(importDbId);
  // A running preview survives closing or reloading this page. Keep the
  // destination so its completed dry-run can still show the confirm step.
  const [confirmImport, setConfirmImport] = useState<DbEntry | null>(null);
  const [importScope, setImportScope] = useState<ImportScope>('all');
  const [importSkipProcessed, setImportSkipProcessed] = useState(false);
  const [importFolders, setImportFolders] = useState<ImportFolder[]>([]);
  // Checked folder paths. Exactly the configured roots means "everything",
  // and the request then omits image_dirs entirely.
  const [importSelectedDirs, setImportSelectedDirs] = useState<string[]>([]);

  const reload = useCallback(async () => {
    setIsLoading(true);
    try {
      // Prefer the Tauri command (returns the full registry including
      // schema_version and active_db_id); fall back to the HTTP listing
      // which gives us enough to render the UI.
      let reg: DbRegistry | null = null;
      const summaries: DatabaseSummary[] = await apiClient.getDatabases();
      if (isTauri) {
        reg = await tauriConfig.getCurrentConfiguration();
        if (reg) {
          reg = {
            ...reg,
            databases: summaries.map((summary) => {
              const persisted = reg?.databases.find(
                (entry) => entry.id === summary.id
              );
              return {
                ...persisted,
                id: summary.id,
                name: summary.name,
                db_path: summary.database_path,
                image_dirs: summary.image_directories,
                export_dir: summary.export_directory,
                remote_image_upload: {
                  enabled: summary.remote_image_upload?.enabled ?? false,
                  image_dir: summary.remote_image_upload?.image_directory,
                  placement: summary.remote_image_upload?.placement ?? 'flat',
                  directory_template:
                    summary.remote_image_upload?.directory_template ??
                    DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE,
                  catalog_directory_template:
                    summary.remote_image_upload?.catalog_directory_template,
                  directory_template_source:
                    summary.remote_image_upload?.directory_template_source ?? 'preset',
                  directory_template_samples:
                    summary.remote_image_upload?.directory_template_samples ?? 0,
                  token_configured:
                    summary.remote_image_upload?.token_configured ?? false,
                  sync_enabled:
                    summary.remote_image_upload?.sync_enabled ?? false,
                  clients: summary.remote_image_upload?.clients ?? [],
                },
              };
            }),
          };
        }
      }
      if (!reg) {
        reg = {
          schema_version: 2,
          databases: summaries.map((s) => ({
            id: s.id,
            name: s.name,
            db_path: s.database_path,
            image_dirs: s.image_directories,
            export_dir: s.export_directory,
            remote_image_upload: {
              enabled: s.remote_image_upload?.enabled ?? false,
              image_dir: s.remote_image_upload?.image_directory,
              placement: s.remote_image_upload?.placement ?? 'flat',
              directory_template:
                s.remote_image_upload?.directory_template ??
                DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE,
              catalog_directory_template:
                s.remote_image_upload?.catalog_directory_template,
              directory_template_source:
                s.remote_image_upload?.directory_template_source ?? 'preset',
              directory_template_samples:
                s.remote_image_upload?.directory_template_samples ?? 0,
              token_configured: s.remote_image_upload?.token_configured ?? false,
              sync_enabled: s.remote_image_upload?.sync_enabled ?? false,
              clients: s.remote_image_upload?.clients ?? [],
            },
          })),
        };
      }
      setRegistry(reg);

      // Import jobs live on the server, while importDbId is only view state.
      // Recover a running job when settings opens so progress polling resumes
      // after a page reload. There can be one job per database; this modal
      // shows the first active job in registry order.
      const runningImport = (
        await Promise.all(
          reg.databases.map(async (entry) => {
            try {
              const status = await apiClient.getImportStatus(entry.id);
              return status.progress.running ? entry : null;
            } catch (err) {
              console.warn(`Failed to check import status for ${entry.id}:`, err);
              return null;
            }
          })
        )
      ).find((entry): entry is DbEntry => entry !== null);
      if (runningImport) {
        setImportDbId(runningImport.id);
        setConfirmImport(runningImport);
      }

      // Deliberately no form is forced open on an empty registry. Doing that
      // picked "add an existing N.I.N.A. database" for the user and, because
      // the choice buttons hide behind an open form, removed "build one from
      // image folders" from the first run altogether. The welcome banner
      // offers both instead.
    } catch (err) {
      console.error('Failed to load registry:', err);
    } finally {
      setIsLoading(false);
    }
  }, [isTauri]);

  useEffect(() => {
    if (!isOpen) return;
    reload();
    setStatusMessage('');
  }, [isOpen, reload]);

  const resetForm = () => {
    setEditingId(null);
    setFormName('');
    setFormDbPath('');
    setFormImageDirs([]);
    setFormRemoteUploadEnabled(false);
    setFormRemoteUploadDir('');
    setFormRemoteUploadPlacement('flat');
    setFormRemoteUploadDirectoryTemplate(
      DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE
    );
    setFormRemoteUploadCatalogMatch(null);
    setFormRemoteUploadLayoutChoice('preset');
    setFormRemoteUploadRescanRequested(false);
    setFormRemoteUploadToken('');
    setFormRemoteUploadTokenConfigured(false);
    setFormRemoteUploadTokenRevealed(false);
    setFormRemoteUploadTokenCopyState('idle');
    setShowAddForm(false);
    setCreateMode(false);
    setCreateAnalyzeQuality(false);
  };

  const startEdit = (entry: DbEntry) => {
    setEditingId(entry.id);
    // A pairing code is scoped to one database; never show it under another.
    setPairingCode(null);
    setPairingExpiresAt(null);
    setPairingCopyState('idle');
    setFormName(entry.name);
    setFormDbPath(entry.db_path);
    setFormImageDirs(entry.image_dirs);
    setFormExportDir(entry.export_dir ?? '');
    setFormRemoteUploadEnabled(entry.remote_image_upload?.enabled ?? false);
    setFormRemoteSyncEnabled(entry.remote_image_upload?.sync_enabled ?? false);
    setFormRemoteUploadDir(
      entry.remote_image_upload?.image_dir ?? entry.image_dirs[0] ?? ''
    );
    setFormRemoteUploadPlacement(
      entry.remote_image_upload?.placement ?? 'flat'
    );
    const directoryTemplate =
      entry.remote_image_upload?.directory_template ??
      DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE;
    setFormRemoteUploadDirectoryTemplate(directoryTemplate);
    setFormRemoteUploadLayoutChoice(
      entry.remote_image_upload?.directory_template_source === 'catalog'
        ? 'catalog'
        : 'preset'
    );
    setFormRemoteUploadCatalogMatch(
      entry.remote_image_upload?.directory_template_source === 'catalog'
        ? {
            template:
              entry.remote_image_upload.catalog_directory_template ??
              directoryTemplate,
            samples: entry.remote_image_upload.directory_template_samples ?? 0,
          }
        : null
    );
    setFormRemoteUploadRescanRequested(false);
    setFormRemoteUploadToken('');
    setFormRemoteUploadTokenConfigured(
      entry.remote_image_upload?.token_configured ??
        Boolean(entry.remote_image_upload?.token_sha256)
    );
    setFormRemoteUploadTokenRevealed(false);
    setFormRemoteUploadTokenCopyState('idle');
    setShowAddForm(true);
    setCreateMode(false);
  };

  const startCreate = () => {
    setEditingId(null);
    setFormName('');
    setFormDbPath('');
    setFormImageDirs([]);
    setFormRemoteUploadEnabled(false);
    setFormRemoteUploadDir('');
    setFormRemoteUploadPlacement('flat');
    setFormRemoteUploadDirectoryTemplate(
      DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE
    );
    setFormRemoteUploadCatalogMatch(null);
    setFormRemoteUploadLayoutChoice('preset');
    setFormRemoteUploadRescanRequested(false);
    setFormRemoteUploadToken('');
    setFormRemoteUploadTokenConfigured(false);
    setFormRemoteUploadTokenRevealed(false);
    setFormRemoteUploadTokenCopyState('idle');
    setShowAddForm(true);
    setCreateMode(true);
    setCreateAnalyzeQuality(false);
  };

  const startAdd = async () => {
    setEditingId(null);
    setFormName('');
    setFormImageDirs([]);
    setFormRemoteUploadEnabled(false);
    setFormRemoteUploadDir('');
    setFormRemoteUploadPlacement('flat');
    setFormRemoteUploadDirectoryTemplate(
      DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE
    );
    setFormRemoteUploadCatalogMatch(null);
    setFormRemoteUploadLayoutChoice('preset');
    setFormRemoteUploadRescanRequested(false);
    setFormRemoteUploadToken('');
    setFormRemoteUploadTokenConfigured(false);
    setFormRemoteUploadTokenRevealed(false);
    setFormRemoteUploadTokenCopyState('idle');
    setShowAddForm(true);
    setCreateMode(false);
    setFormDbPath('');

    if (isTauri) {
      // Try to seed with the default N.I.N.A. database path (Windows only).
      try {
        const def = await tauriFileSystem.getDefaultNinaPath();
        if (def) setFormDbPath(def);
      } catch {
        // Ignore — the form just stays empty.
      }
    }
  };

  // Land on the form the caller asked for. This runs once, at mount: App
  // unmounts the modal when it closes, so re-opening with a fresh intent
  // mounts a fresh component and the effect fires again.
  useEffect(() => {
    if (initialIntent === 'create') startCreate();
    if (initialIntent === 'add') void startAdd();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handlePickDbPath = async () => {
    if (!isTauri) {
      setStatusMessage(
        'File picker is only available in the desktop app — paste the path into the field.'
      );
      return;
    }
    try {
      const path = await tauriFileSystem.pickDatabaseFile();
      if (path) setFormDbPath(path);
    } catch (err) {
      console.error('pickDatabaseFile failed:', err);
    }
  };

  const handleAddImageDir = async () => {
    if (!isTauri) {
      setStatusMessage(
        'Image directory picker is only available in the desktop app — type the path below and press the Add button.'
      );
      return;
    }
    try {
      const path = await tauriFileSystem.pickImageDirectory();
      if (path && !formImageDirs.includes(path)) {
        setFormImageDirs([...formImageDirs, path]);
      }
    } catch (err) {
      console.error('pickImageDirectory failed:', err);
    }
  };

  // Browser-mode fallback: manually add an image directory from a text input.
  const [pendingImageDir, setPendingImageDir] = useState('');
  const handleAddManualImageDir = () => {
    const trimmed = pendingImageDir.trim();
    if (trimmed && !formImageDirs.includes(trimmed)) {
      setFormImageDirs([...formImageDirs, trimmed]);
      setPendingImageDir('');
    }
  };

  const handleRemoveImageDir = (index: number) => {
    const removed = formImageDirs[index];
    const remaining = formImageDirs.filter((_, i) => i !== index);
    setFormImageDirs(remaining);
    if (formRemoteUploadDir === removed) {
      setFormRemoteUploadDir(remaining[0] ?? '');
    }
  };

  const [pairingCode, setPairingCode] = useState<string | null>(null);
  const [pairingExpiresAt, setPairingExpiresAt] = useState<number | null>(null);
  const [pairingCopyState, setPairingCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');

  const handleIssuePairingCode = async () => {
    if (!editingId) return;
    try {
      const issued = await apiClient.issuePairingCode(editingId);
      setPairingCode(issued.pairing_token);
      setPairingExpiresAt(issued.expires_at);
      setPairingCopyState('idle');
    } catch (error) {
      setStatusMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const handleRevokeClient = async (clientUuid: string) => {
    if (!editingId) return;
    try {
      await apiClient.revokePairedClient(editingId, clientUuid);
      // The databases list is component state fed by reload(), not a
      // react-query cache — invalidation alone would leave the revoked
      // client on screen.
      await reload();
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      setStatusMessage('Client revoked');
    } catch (error) {
      setStatusMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const handleCopyPairingCode = async () => {
    if (!pairingCode) return;
    try {
      await navigator.clipboard.writeText(pairingCode);
      setPairingCopyState('copied');
    } catch {
      setPairingCopyState('failed');
    }
  };

  const handleGenerateRemoteUploadToken = () => {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);
    const token = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join(
      ''
    );
    setFormRemoteUploadToken(token);
    setFormRemoteUploadTokenRevealed(true);
    setFormRemoteUploadTokenCopyState('idle');
  };

  const handleCopyRemoteUploadToken = async () => {
    try {
      await navigator.clipboard.writeText(formRemoteUploadToken);
      setFormRemoteUploadTokenCopyState('copied');
    } catch (error) {
      console.error('Failed to copy remote upload token:', error);
      setFormRemoteUploadTokenCopyState('failed');
    }
  };

  const handleSaveForm = async () => {
    if (createMode) {
      // "New database from images": the server bootstraps a fresh TS-schema
      // database and imports the folders in the background.
      if (formImageDirs.length === 0) {
        setStatusMessage('Add at least one image directory to import');
        return;
      }
      const name = formName.trim() || 'Imported Images';
      setIsApplying(true);
      setStatusMessage('');
      try {
        const created = await apiClient.createDatabaseFromImages({
          name,
          image_dirs: formImageDirs,
          backfill: createAnalyzeQuality,
          fill_metadata: starMetadataFillEnabled(),
        });
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db'] });
        setImportDbId(created.database.id);
        await reload();
        resetForm();
        setStatusMessage(`Created ${created.database.name}; importing images…`);
      } catch (err) {
        console.error('create-from-images failed:', err);
        const msg = err instanceof Error ? err.message : String(err);
        setStatusMessage(`Failed to create: ${msg}`);
      } finally {
        setIsApplying(false);
      }
      return;
    }

    if (!formDbPath.trim()) {
      setStatusMessage('Please select a database file');
      return;
    }

    const inferredName =
      formName.trim() ||
      formDbPath.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') ||
      'Database';

    setIsApplying(true);
    setStatusMessage('');

    try {
      // Use HTTP endpoints — they're available in both Tauri and CLI-server
      // mode, and updating live `AppState.databases` rather than waiting for
      // a server restart.
      if (editingId) {
        if (formRemoteUploadEnabled && !formRemoteUploadDir) {
          setStatusMessage('Select an image directory for remote uploads');
          setIsApplying(false);
          return;
        }
        if (
          formRemoteUploadEnabled &&
          formRemoteUploadPlacement === 'target_tree' &&
          formRemoteUploadLayoutChoice === 'preset' &&
          !formRemoteUploadDirectoryTemplate.trim()
        ) {
          setStatusMessage('Enter a folder layout');
          setIsApplying(false);
          return;
        }
        if (
          formRemoteUploadToken.length > 0 &&
          formRemoteUploadToken.length < 24
        ) {
          setStatusMessage('Remote API key must be at least 24 characters');
          setIsApplying(false);
          return;
        }
        if (
          formRemoteUploadEnabled &&
          !formRemoteUploadTokenConfigured &&
          formRemoteUploadToken.length === 0
        ) {
          setStatusMessage('Generate a remote API key before enabling image uploads');
          setIsApplying(false);
          return;
        }
        if (
          formRemoteSyncEnabled &&
          !formRemoteUploadTokenConfigured &&
          formRemoteUploadToken.length === 0
        ) {
          setStatusMessage('Generate a remote API key before enabling scheduler sync');
          setIsApplying(false);
          return;
        }
        await apiClient.updateDatabase(editingId, {
          name: inferredName,
          db_path: formDbPath.trim(),
          image_dirs: formImageDirs,
          export_dir: formExportDir.trim(),
          remote_image_upload: {
            enabled: formRemoteUploadEnabled,
            image_directory: formRemoteUploadDir || undefined,
            token: formRemoteUploadToken || undefined,
            sync_enabled: formRemoteSyncEnabled,
            placement: formRemoteUploadPlacement,
            directory_template:
              formRemoteUploadEnabled &&
              formRemoteUploadPlacement === 'target_tree'
                ? formRemoteUploadDirectoryTemplate.trim() ||
                  DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE
                : undefined,
            directory_template_source:
              formRemoteUploadEnabled &&
              formRemoteUploadPlacement === 'target_tree'
                ? formRemoteUploadLayoutChoice
                : undefined,
            rescan_directory_layout:
              formRemoteUploadEnabled &&
              formRemoteUploadPlacement === 'target_tree' &&
              formRemoteUploadRescanRequested
                ? true
                : undefined,
          },
        });
      } else {
        await apiClient.addDatabase({
          name: inferredName,
          db_path: formDbPath.trim(),
          image_dirs: formImageDirs,
        });
      }

      // Invalidate every per-DB query so the merged-overview hooks pull
      // fresh data for the just-added/edited DB.
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      queryClient.invalidateQueries({ queryKey: ['db'] });

      await reload();
      resetForm();
      setStatusMessage('Saved.');
    } catch (err) {
      console.error('save failed:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to save: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  // Import is two-step: a dry-run PREVIEW first (rolled back server-side),
  // then an explicit confirmation. Nothing touches the database until the
  // user has seen exactly what would be attached vs newly created.
  // Scope, folder, and artifact choices as request fields. Selection equal
  // to the configured roots means "everything" and sends no image_dirs, so
  // the default run needs no folder consent beyond what is configured.
  const importOptionsOf = (entry: DbEntry) => {
    const roots = [...entry.image_dirs].sort();
    const selection = [...importSelectedDirs].sort();
    const isDefaultSelection =
      selection.length === roots.length && selection.every((dir, i) => dir === roots[i]);
    return {
      scope: importScope === 'all' ? undefined : importScope,
      skip_processed: importSkipProcessed || undefined,
      image_dirs: isDefaultSelection || selection.length === 0 ? undefined : selection,
    };
  };

  const previewImport = async (entry: DbEntry, options: ReturnType<typeof importOptionsOf>) => {
    const status = await apiClient.startImport(entry.id, {
      dry_run: true,
      backfill: false,
      ...options,
    });
    setImportDbId(entry.id);
    setConfirmImport(entry);
    setStatusMessage(
      status.started
        ? `Previewing import into ${entry.name}… nothing is written until you confirm.`
        : 'An import is already running for this database.'
    );
  };

  const handleImport = async (entry: DbEntry) => {
    if (entry.image_dirs.length === 0) {
      setStatusMessage(
        `"${entry.name}" has no image directories configured — edit it and add the folders to import.`
      );
      return;
    }
    setIsApplying(true);
    setStatusMessage('');
    setImportAnalyzeQuality(false);
    setImportScope('all');
    setImportSkipProcessed(false);
    setImportSelectedDirs([...entry.image_dirs]);
    setImportFolders([]);
    apiClient
      .getImportFolders(entry.id)
      .then(setImportFolders)
      .catch(() => setImportFolders([]));
    try {
      await previewImport(entry, {
        scope: undefined,
        skip_processed: undefined,
        image_dirs: undefined,
      });
    } catch (err) {
      console.error('import preview failed to start:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to preview import: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleRepreviewImport = async () => {
    if (!confirmImport) return;
    setIsApplying(true);
    try {
      await previewImport(confirmImport, importOptionsOf(confirmImport));
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to preview import: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleConfirmImport = async () => {
    if (!confirmImport) return;
    const entry = confirmImport;
    setConfirmImport(null);
    setIsApplying(true);
    try {
      await apiClient.startImport(entry.id, {
        dry_run: false,
        backfill: importAnalyzeQuality,
        fill_metadata: starMetadataFillEnabled(),
        ...importOptionsOf(entry),
      });
      setStatusMessage(`Importing images into ${entry.name}…`);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to start import: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleRemove = async (entry: DbEntry) => {
    if (!confirm(`Remove "${entry.name}" from the configured databases?`)) return;
    setIsApplying(true);
    try {
      const ok = await apiClient.removeDatabase(entry.id);
      if (ok) {
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db', entry.id] });
        await reload();
        if (editingId === entry.id) resetForm();
        setStatusMessage(`Removed ${entry.name}.`);
      } else {
        setStatusMessage('Remove failed.');
      }
    } catch (err) {
      console.error('remove failed:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to remove: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  // CRUD changes are applied to the live server immediately (HTTP endpoints
  // update both registry file and AppState.databases). The restart button is
  // only useful in rare cases where the live state diverged from disk — keep
  // it as an opt-in escape hatch in Tauri mode.
  const handleRestart = async () => {
    if (!isTauri) {
      setStatusMessage('Refreshing interface...');
      setTimeout(() => window.location.reload(), 800);
      return;
    }
    setIsApplying(true);
    setStatusMessage('Restarting server...');
    try {
      const restarted = await tauriConfig.restartServer();
      if (restarted) {
        setStatusMessage('Restarting interface...');
        setTimeout(() => window.location.reload(), 1500);
      } else {
        setStatusMessage('Server restart failed; falling back to app restart...');
        setTimeout(() => tauriConfig.restartApplication(), 1500);
      }
    } catch (err) {
      console.error('restart failed:', err);
      setStatusMessage(`Restart failed: ${err}`);
      setIsApplying(false);
    }
  };

  if (!isOpen) return null;

  const databases = registry?.databases ?? [];
  const hasDatabases = databases.length > 0;

  // Catalogs are management-gated, and sync has nothing to talk about until a
  // catalog exists — the same conditions that used to hide those sections.
  const visibleTabs: Array<{ id: SettingsTab; label: string }> = [
    { id: 'databases', label: 'Databases' },
    ...(managementAllowed
      ? ([{ id: 'catalogs', label: 'Catalogs' }] as const)
      : []),
    ...(managementAllowed && hasDatabases
      ? ([{ id: 'sync', label: 'Sync' }] as const)
      : []),
    // Setups are global display parameters, not filesystem paths, so the tab
    // is not management-gated.
    { id: 'setups', label: 'Setups' },
    // Review preferences are stored in this browser, so no gate either.
    { id: 'review', label: 'Review' },
    ...(!isTauri && access.status.authentication_required
      ? ([{ id: 'users', label: 'Users' }] as const)
      : []),
  ];
  // Derive rather than store: removing the last database takes the Sync tab
  // away, and the selection has to fall back in the same render.
  const currentTab = visibleTabs.some((tab) => tab.id === activeTab)
    ? activeTab
    : 'databases';

  const onTabKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step =
      event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const index = visibleTabs.findIndex((tab) => tab.id === currentTab);
    const next = visibleTabs[(index + step + visibleTabs.length) % visibleTabs.length];
    setActiveTab(next.id);
    document.getElementById(`settings-tab-${next.id}`)?.focus();
  };

  const renderDatabaseForm = (entry: DbEntry | null) => {
    const editorHeadingId = entry ? `database-editor-heading-${entry.id}` : undefined;

    return (
      <section
        className={entry ? 'db-row-editor' : 'settings-section'}
        aria-labelledby={editorHeadingId}
      >
        {entry ? (
          <div className="db-editor-heading">
            <span className="db-editor-context">Editing database</span>
            <h3 id={editorHeadingId}>
              {entry.name} <code className="db-row-slug">{entry.id}</code>
            </h3>
          </div>
        ) : (
          <h3>{createMode ? 'New Database from Images' : 'Add Database'}</h3>
        )}

        {createMode && (
          <p className="muted">
            Creates a brand-new Target Scheduler database and imports the
            selected folders. Each target gets its own project. Nearby,
            similarly dated panels with matching panel names share a
            mosaic project. You can rename or reorganize them afterwards.
            The import reads headers only; pixel-based quality work is a
            separate option below.
          </p>
        )}

        <div className="database-config">
          <label>Display name (optional):</label>
          <input
            type="text"
            value={formName}
            onChange={(e) => setFormName(e.target.value)}
            placeholder={
              createMode
                ? 'e.g. 2026 Archive (defaults to "Imported Images")'
                : 'e.g. Imaging Rig (defaults to filename)'
            }
            className="file-path-input"
          />
        </div>

        {!createMode && (
          <div className="database-config">
            <label>N.I.N.A. Database File:</label>
            <div className="file-input-group">
              <input
                type="text"
                value={formDbPath}
                onChange={(e) => setFormDbPath(e.target.value)}
                placeholder="Select or enter database path"
                className="file-path-input"
              />
              <button onClick={handlePickDbPath} className="browse-button">
                Browse…
              </button>
            </div>
          </div>
        )}

        <div className="database-config">
          <label>Image Directories:</label>
          {isTauri ? (
            <button onClick={handleAddImageDir} className="add-directory-button">
              + Add Image Directory
            </button>
          ) : (
            <div className="file-input-group">
              <input
                type="text"
                value={pendingImageDir}
                onChange={(e) => setPendingImageDir(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    handleAddManualImageDir();
                  }
                }}
                placeholder="Type an absolute path and press Add"
                className="file-path-input"
              />
              <button
                onClick={handleAddManualImageDir}
                className="browse-button"
                disabled={!pendingImageDir.trim()}
              >
                Add
              </button>
            </div>
          )}
          {formImageDirs.length > 0 && (
            <div className="image-directories">
              {formImageDirs.map((dir, index) => (
                <div key={dir} className="image-directory-item">
                  <span>📂 {dir}</span>
                  <button
                    onClick={() => handleRemoveImageDir(index)}
                    className="remove-button"
                    title="Remove directory"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        {editingId && (
          <div className="database-config">
            <label>Pair a client:</label>
            <div className="remote-upload-token-row">
              <button
                type="button"
                onClick={handleIssuePairingCode}
                className="browse-button"
              >
                Generate pairing code
              </button>
              {pairingCode && (
                <>
                  <input
                    type="text"
                    readOnly
                    value={pairingCode}
                    className="file-path-input"
                    onFocus={(event) => event.target.select()}
                  />
                  <button
                    type="button"
                    onClick={handleCopyPairingCode}
                    className="browse-button"
                  >
                    {pairingCopyState === 'copied' ? 'Copied' : 'Copy'}
                  </button>
                </>
              )}
            </div>
            {pairingCode && (
              <small className="remote-upload-token-notice" role="status">
                {pairingCopyState === 'failed'
                  ? 'Copy failed. Select and copy the code manually.'
                  : `Paste this code into the client. One use, expires ${
                      pairingExpiresAt
                        ? new Date(pairingExpiresAt * 1000).toLocaleTimeString()
                        : 'in an hour'
                    }. Each pairing adds its own revocable credential.`}
              </small>
            )}
            {(databases.find((db) => db.id === editingId)?.remote_image_upload
              ?.clients?.length ?? 0) > 0 && (
              <div className="paired-clients">
                <label>Paired clients:</label>
                <ul>
                  {databases
                    .find((db) => db.id === editingId)!
                    .remote_image_upload!.clients!.map((client) => (
                      <li key={client.client_uuid}>
                        <span>
                          {client.name} ·{' '}
                          {new Date(client.paired_at * 1000).toLocaleDateString()}
                        </span>
                        <button
                          type="button"
                          className="browse-button"
                          onClick={() => handleRevokeClient(client.client_uuid)}
                        >
                          Revoke
                        </button>
                      </li>
                    ))}
                </ul>
              </div>
            )}
            <label htmlFor="remote-upload-token">Remote API key:</label>
            <div className="remote-upload-token-row">
              <input
                id="remote-upload-token"
                type={formRemoteUploadTokenRevealed ? 'text' : 'password'}
                value={formRemoteUploadToken}
                onChange={(event) => {
                  setFormRemoteUploadToken(event.target.value);
                  setFormRemoteUploadTokenCopyState('idle');
                }}
                placeholder={
                  formRemoteUploadTokenConfigured
                    ? 'Unchanged'
                    : 'At least 24 characters'
                }
                className="file-path-input"
                autoComplete="new-password"
              />
              <button
                type="button"
                onClick={handleGenerateRemoteUploadToken}
                className="browse-button"
              >
                Generate
              </button>
              {formRemoteUploadTokenRevealed &&
                formRemoteUploadToken.length > 0 && (
                  <button
                    type="button"
                    onClick={handleCopyRemoteUploadToken}
                    className="browse-button"
                  >
                    {formRemoteUploadTokenCopyState === 'copied'
                      ? 'Copied'
                      : 'Copy'}
                  </button>
                )}
            </div>
            {formRemoteUploadTokenRevealed && (
              <small
                className={`remote-upload-token-notice ${
                  formRemoteUploadTokenCopyState === 'failed' ? 'error' : ''
                }`}
                role="status"
              >
                {formRemoteUploadTokenCopyState === 'failed'
                  ? 'Copy failed. Select and copy the key manually.'
                  : 'Copy this key now. It will not be shown again after saving.'}
              </small>
            )}
            <label className="quality-analysis-option">
              <input
                type="checkbox"
                checked={formRemoteSyncEnabled}
                onChange={(event) =>
                  setFormRemoteSyncEnabled(event.target.checked)
                }
              />
              <span>
                <strong>Accept remote scheduler sync</strong>
                <small>
                  Lets a holder of this key merge projects, targets,
                  plans, and grades into this database.
                </small>
              </span>
            </label>
            <label className="quality-analysis-option">
              <input
                type="checkbox"
                checked={formRemoteUploadEnabled}
                onChange={(event) =>
                  setFormRemoteUploadEnabled(event.target.checked)
                }
              />
              <span>
                <strong>Accept remote image uploads</strong>
              </span>
            </label>
            {formRemoteUploadEnabled && (
              <>
                <label htmlFor="remote-upload-directory">Receive directory:</label>
                <select
                  id="remote-upload-directory"
                  value={formRemoteUploadDir}
                  onChange={(event) => setFormRemoteUploadDir(event.target.value)}
                  className="file-path-input"
                >
                  <option value="">Select an image directory</option>
                  {formImageDirs.map((directory) => (
                    <option key={directory} value={directory}>
                      {directory}
                    </option>
                  ))}
                </select>
                <label htmlFor="remote-upload-placement">Folder layout:</label>
                <select
                  id="remote-upload-placement"
                  value={formRemoteUploadPlacement}
                  onChange={(event) =>
                    setFormRemoteUploadPlacement(
                      event.target.value as RemoteImageUploadPlacement
                    )
                  }
                  className="file-path-input"
                >
                  <option value="flat">Receive directory</option>
                  <option value="target_tree">Match catalog</option>
                </select>
                {formRemoteUploadPlacement === 'target_tree' && (
                  <>
                    {formRemoteUploadCatalogMatch && (
                      <small className="remote-upload-token-notice" role="status">
                        Detected from {formRemoteUploadCatalogMatch.samples}{' '}
                        catalog {formRemoteUploadCatalogMatch.samples === 1
                          ? 'image'
                          : 'images'}:{' '}
                        <code>{formRemoteUploadCatalogMatch.template}</code>
                      </small>
                    )}
                    <label htmlFor="remote-upload-directory-template">
                      Layout:
                    </label>
                    <div className="file-input-group">
                      <select
                        id="remote-upload-directory-template"
                        value={
                          formRemoteUploadLayoutChoice === 'catalog' &&
                          formRemoteUploadCatalogMatch
                            ? 'catalog'
                            : isRemoteUploadDirectoryTemplatePreset(
                                  formRemoteUploadDirectoryTemplate
                                )
                              ? formRemoteUploadDirectoryTemplate
                              : 'custom'
                        }
                        onChange={(event) => {
                          const choice = event.target.value;
                          if (choice === 'catalog') {
                            setFormRemoteUploadLayoutChoice('catalog');
                            return;
                          }
                          setFormRemoteUploadLayoutChoice('preset');
                          setFormRemoteUploadDirectoryTemplate(
                            choice === 'custom' ? '' : choice
                          );
                        }}
                        className="file-path-input"
                      >
                        {formRemoteUploadCatalogMatch && (
                          <option value="catalog">
                            Match catalog: {formRemoteUploadCatalogMatch.template}
                          </option>
                        )}
                        {REMOTE_UPLOAD_DIRECTORY_TEMPLATE_PRESETS.map((template) => (
                          <option key={template} value={template}>
                            {template}
                          </option>
                        ))}
                        <option value="custom">Custom</option>
                      </select>
                      <button
                        type="button"
                        className="browse-button"
                        aria-label="Rescan catalog layout"
                        aria-pressed={formRemoteUploadRescanRequested}
                        title={
                          formRemoteUploadRescanRequested
                            ? 'Catalog layout rescan queued for Save Changes'
                            : 'Rescan catalog paths when changes are saved'
                        }
                        disabled={formRemoteUploadRescanRequested}
                        onClick={() => setFormRemoteUploadRescanRequested(true)}
                      >
                        ↻ {formRemoteUploadRescanRequested ? 'Rescan queued' : 'Rescan'}
                      </button>
                    </div>
                    {formRemoteUploadLayoutChoice === 'preset' &&
                      !isRemoteUploadDirectoryTemplatePreset(
                        formRemoteUploadDirectoryTemplate
                      ) && (
                      <input
                        type="text"
                        aria-label="Custom catalog layout:"
                        className="file-path-input"
                        value={formRemoteUploadDirectoryTemplate}
                        onChange={(event) =>
                          setFormRemoteUploadDirectoryTemplate(event.target.value)
                        }
                        placeholder="%TARGET%/%NIGHT%/%TYPE%"
                        maxLength={240}
                      />
                    )}
                  </>
                )}
              </>
            )}
            <label htmlFor="server-export-directory">
              Server export directory:
            </label>
            <input
              id="server-export-directory"
              type="text"
              className="file-path-input"
              placeholder="Absolute server path; empty disables server export"
              title="Exports triggered from the Overview land here (reflinked where the filesystem supports it). Leave empty to offer the archive download instead."
              value={formExportDir}
              onChange={(event) => setFormExportDir(event.target.value)}
            />
          </div>
        )}

        {createMode && (
          <label className="quality-analysis-option">
            <input
              type="checkbox"
              checked={createAnalyzeQuality}
              onChange={(event) => setCreateAnalyzeQuality(event.target.checked)}
            />
            <span>
              <strong>Queue background quality analysis</strong>
              <small>
                Reads every image to measure stars, background, clouds, obstructions,
                and pointing. This can take a long time, especially in a debug build.
                You can run it later from this database&apos;s settings.
              </small>
            </span>
          </label>
        )}

        <div className="modal-buttons">
          <button
            onClick={resetForm}
            className="cancel-button"
            disabled={isApplying}
          >
            Cancel
          </button>
          <button
            onClick={handleSaveForm}
            className="save-button"
            disabled={
              (createMode ? formImageDirs.length === 0 : !formDbPath.trim()) || isApplying
            }
          >
            {createMode
              ? 'Create & Import'
              : editingId
                ? 'Save Changes'
                : 'Add Database'}
          </button>
        </div>
      </section>
    );
  };

  return (
    <div className="tauri-settings modal-overlay" onClick={onClose}>
      <div className="modal-content" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h2>PSF Guard Settings</h2>
          <button className="close-button" onClick={onClose}>×</button>
        </div>

        <div
          className="settings-tabs"
          role="tablist"
          aria-label="Settings sections"
          onKeyDown={onTabKeyDown}
        >
          {visibleTabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              id={`settings-tab-${tab.id}`}
              aria-selected={tab.id === currentTab}
              aria-controls={`settings-panel-${tab.id}`}
              tabIndex={tab.id === currentTab ? 0 : -1}
              className={`settings-tab${tab.id === currentTab ? ' active' : ''}`}
              onClick={() => setActiveTab(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div
          className="modal-body"
          role="tabpanel"
          id={`settings-panel-${currentTab}`}
          aria-labelledby={`settings-tab-${currentTab}`}
        >
          {currentTab === 'review' && <ReviewPreferences />}
          {currentTab === 'databases' && (
          <>
          {!hasDatabases && managementAllowed && (
            <div className="welcome-message">
              <h3>🚀 Welcome to PSF Guard!</h3>
              <p>
                Start from folders of FITS or XISF images, or from a N.I.N.A.
                scheduler database you already have. You can add more catalogs
                later.
              </p>
              {!showAddForm && (
                <div className="welcome-choices">
                  <button
                    className="add-directory-button"
                    onClick={startCreate}
                    disabled={isApplying}
                  >
                    <span className="welcome-choice-title">
                      ✨ New Database from Images
                    </span>
                    <span className="welcome-choice-detail">
                      Pick folders of FITS or XISF files. PSF Guard builds a
                      Target Scheduler database and imports them.
                    </span>
                  </button>
                  <button
                    className="add-directory-button"
                    onClick={startAdd}
                    disabled={isApplying}
                  >
                    <span className="welcome-choice-title">
                      + Add Existing Database
                    </span>
                    <span className="welcome-choice-detail">
                      Open a N.I.N.A. Target Scheduler database and point it at
                      your image folders.
                    </span>
                  </button>
                </div>
              )}
            </div>
          )}

          {!managementAllowed && (
            <div className="welcome-message" style={{ borderColor: 'var(--color-border-warning, #c62)' }}>
              <h3>🔒 Database management is read-only</h3>
              <p>
                This server was launched without
                <code style={{ margin: '0 4px' }}>--allow-database-management</code>,
                so the configured database list cannot be changed from the
                browser. Add databases on the command line —
                <code style={{ margin: '0 4px' }}>
                  psf-guard server &lt;db&gt; &lt;image-dirs…&gt;
                </code>
                — or restart the server with the flag to enable add/edit/remove
                here.
              </p>
            </div>
          )}

          <div className="settings-section">
            <h3>Configured Databases {hasDatabases && <span className="muted">({databases.length})</span>}</h3>

            {isLoading && <div className="detecting-database">Loading…</div>}

            {!isLoading && databases.length === 0 && !showAddForm && (
              <div className="no-directories">
                No databases configured yet.
              </div>
            )}

            {databases.map((entry) => {
              const isEditing = showAddForm && editingId === entry.id;
              const editorId = `database-editor-${entry.id}`;

              return (
                <section
                  key={entry.id}
                  className={`db-entry${isEditing ? ' editing' : ''}`}
                  aria-labelledby={`database-heading-${entry.id}`}
                >
                  <div className="db-row">
                    <div className="db-row-main">
                      <div className="db-row-title">
                        <strong id={`database-heading-${entry.id}`}>{entry.name}</strong>{' '}
                        <code className="db-row-slug">{entry.id}</code>
                      </div>
                      <div className="path-info">{entry.db_path}</div>
                      {entry.image_dirs.length > 0 && (
                        <div className="path-info muted">
                          {entry.image_dirs.join(', ')}
                        </div>
                      )}
                      {entry.remote_image_upload?.enabled && (
                        <div className="path-info muted">
                          Remote receive: {entry.remote_image_upload.image_dir} (
                          {entry.remote_image_upload.placement === 'target_tree'
                            ? entry.remote_image_upload.directory_template_source ===
                              'catalog'
                              ? `match catalog, ${entry.remote_image_upload.directory_template_samples ?? 0} ${entry.remote_image_upload.directory_template_samples === 1 ? 'sample' : 'samples'}`
                              : 'match catalog'
                            : 'receive directory'}
                          )
                        </div>
                      )}
                      <CalibrationLibrarySummary
                        dbId={entry.id}
                        dbName={entry.name}
                        canManage={managementAllowed}
                        onImport={() => handleImport(entry)}
                      />
                      <QualityBackfillControls dbId={entry.id} />
                    </div>
                    {managementAllowed && (
                      <div className="db-row-actions">
                        <button
                          className="browse-button"
                          onClick={() => handleImport(entry)}
                          disabled={isApplying || (importRunning && importDbId === entry.id)}
                          title="Scan this database's image directories and import new frames"
                        >
                          {importRunning && importDbId === entry.id ? 'Importing…' : 'Import'}
                        </button>
                        <button
                          className="browse-button"
                          onClick={() => startEdit(entry)}
                          disabled={isApplying}
                          aria-label={`Edit ${entry.name}`}
                          aria-expanded={isEditing}
                          aria-controls={isEditing ? editorId : undefined}
                        >
                          Edit
                        </button>
                        <button
                          className="remove-button"
                          onClick={() => handleRemove(entry)}
                          disabled={isApplying}
                          title="Remove this database"
                        >
                          Remove
                        </button>
                      </div>
                    )}
                  </div>
                  {isEditing && <div id={editorId}>{renderDatabaseForm(entry)}</div>}
                </section>
              );
            })}

            {/* With no databases yet the welcome banner carries these same two
                actions, so rendering them here too would just duplicate them. */}
            {managementAllowed && hasDatabases && !showAddForm && (
              <div className="db-add-buttons">
                <button
                  className="add-directory-button"
                  onClick={startAdd}
                  disabled={isApplying}
                >
                  + Add Database
                </button>
                <button
                  className="add-directory-button"
                  onClick={startCreate}
                  disabled={isApplying}
                  title="Create a brand-new Target Scheduler database and import folders of images into it"
                >
                  ✨ New Database from Images
                </button>
              </div>
            )}

            {importDbId && importProgress && importProgress.stage !== '' && (
              <div className="import-progress-panel">
                {confirmImport && importDbId === confirmImport.id && (
                  <div className="import-scope-controls">
                    <label className="import-scope-choice">
                      Import
                      <select
                        value={importScope}
                        onChange={(event) => setImportScope(event.target.value as ImportScope)}
                        disabled={importRunning || isApplying}
                      >
                        <option value="all">Lights and calibration</option>
                        <option value="lights">Lights only</option>
                        <option value="calibration">Calibration only</option>
                      </select>
                    </label>
                    {importFolders.length > 0 && (
                      <div className="import-folder-tree">
                        {importFolders.map((root) => (
                          <details key={root.path} className="import-folder-root">
                            <summary>
                              <label onClick={(event) => event.stopPropagation()}>
                                <input
                                  type="checkbox"
                                  checked={importSelectedDirs.includes(root.path)}
                                  onChange={() =>
                                    setImportSelectedDirs((current) =>
                                      current.includes(root.path)
                                        ? current.filter((d) => d !== root.path)
                                        : [...current, root.path]
                                    )
                                  }
                                />
                                {root.path}
                              </label>
                            </summary>
                            {root.children.map((child) => (
                              <div key={child.path} className="import-folder-child">
                                <label>
                                  <input
                                    type="checkbox"
                                    checked={
                                      importSelectedDirs.includes(child.path) ||
                                      importSelectedDirs.includes(root.path)
                                    }
                                    disabled={importSelectedDirs.includes(root.path)}
                                    onChange={() =>
                                      setImportSelectedDirs((current) =>
                                        current.includes(child.path)
                                          ? current.filter((d) => d !== child.path)
                                          : [...current, child.path]
                                      )
                                    }
                                  />
                                  {child.name}
                                </label>
                                {child.children.length > 0 && (
                                  <div className="import-folder-grandchildren">
                                    {child.children.map((grandchild) => (
                                      <label key={grandchild.path}>
                                        <input
                                          type="checkbox"
                                          checked={
                                            importSelectedDirs.includes(grandchild.path) ||
                                            importSelectedDirs.includes(child.path) ||
                                            importSelectedDirs.includes(root.path)
                                          }
                                          disabled={
                                            importSelectedDirs.includes(child.path) ||
                                            importSelectedDirs.includes(root.path)
                                          }
                                          onChange={() =>
                                            setImportSelectedDirs((current) =>
                                              current.includes(grandchild.path)
                                                ? current.filter((d) => d !== grandchild.path)
                                                : [...current, grandchild.path]
                                            )
                                          }
                                        />
                                        {grandchild.name}
                                      </label>
                                    ))}
                                  </div>
                                )}
                              </div>
                            ))}
                          </details>
                        ))}
                      </div>
                    )}
                    <label className="quality-analysis-option import-skip-processed">
                      <input
                        type="checkbox"
                        checked={importSkipProcessed}
                        onChange={(event) => setImportSkipProcessed(event.target.checked)}
                        disabled={importRunning || isApplying}
                      />
                      <span>
                        <small>
                          Skip processing artifacts (integration masters and
                          calibrated/registered intermediates). Useful when a scanned
                          folder contains a processing tree whose derived files repeat
                          exposures already cataloged.
                        </small>
                      </span>
                    </label>
                    <button
                      className="browse-button"
                      onClick={handleRepreviewImport}
                      disabled={importRunning || isApplying}
                      title="Re-run the preview with the selection above; nothing is written."
                    >
                      Update preview
                    </button>
                  </div>
                )}
                <div className="import-progress-line">
                  {importRunning && <span className="import-spinner">⏳ </span>}
                  {describeImportProgress(importProgress)}
                </div>
                {importProgress.stage === 'complete' && importProgress.outcome && (
                  <>
                    {(importProgress.outcome.skipped_processed ?? 0) > 0 && (
                      <div className="muted">
                        {importProgress.outcome.skipped_processed} processing artifact(s)
                        (masters, calibrated/registered intermediates) skipped.
                      </div>
                    )}
                    {(importProgress.outcome.skipped_out_of_scope ?? 0) > 0 && (
                      <div className="muted">
                        {importProgress.outcome.skipped_out_of_scope} frame(s) outside the
                        selected scope.
                      </div>
                    )}
                    {importProgress.outcome.attach_summaries.length > 0 && (
                      <ul className="import-project-list">
                        {importProgress.outcome.attach_summaries.map((a) => (
                          <li key={`${a.project}:${a.target}`}>
                            ↳ existing {a.project} / {a.target} — +{a.frames} frame(s) (
                            {a.matched_by} match)
                          </li>
                        ))}
                      </ul>
                    )}
                    {importProgress.outcome.project_summaries.length > 0 && (
                      <ul className="import-project-list">
                        {importProgress.outcome.project_summaries.map((p) => (
                          <li key={p.name}>
                            NEW {p.name} — {p.targets} target(s), {p.frames} frame(s)
                          </li>
                        ))}
                      </ul>
                    )}
                    {importProgress.outcome.dry_run &&
                      confirmImport &&
                      importDbId === confirmImport.id && (
                        <div className="modal-buttons import-confirm-buttons">
                          {importProgress.outcome.imported +
                            importProgress.outcome.calibration.imported +
                            importProgress.outcome.calibration.updated >
                          0 ? (
                            <div className="import-confirm-content">
                              {importProgress.outcome.imported > 0 && <label className="quality-analysis-option">
                                <input
                                  type="checkbox"
                                  checked={importAnalyzeQuality}
                                  onChange={(event) =>
                                    setImportAnalyzeQuality(event.target.checked)
                                  }
                                />
                                <span>
                                  <strong>Queue background quality analysis</strong>
                                  <small>
                                    Reads every image to measure stars, background, clouds,
                                    obstructions, and pointing. This can take a long time,
                                    especially in a debug build. You can run it later from this
                                    database&apos;s settings.
                                  </small>
                                </span>
                              </label>}
                              <div className="modal-buttons import-action-buttons">
                              <button
                                className="save-button"
                                onClick={handleConfirmImport}
                                disabled={isApplying}
                              >
                                Import{' '}
                                {importProgress.outcome.imported +
                                  importProgress.outcome.calibration.imported +
                                  importProgress.outcome.calibration.updated}{' '}
                                frame(s)
                              </button>
                              <button
                                className="cancel-button"
                                onClick={() => {
                                  setConfirmImport(null);
                                  setStatusMessage('Import cancelled — nothing was written.');
                                }}
                                disabled={isApplying}
                              >
                                Cancel
                              </button>
                              </div>
                            </div>
                          ) : (
                            <span className="muted">
                              Nothing new to import — every frame is already in the database.
                            </span>
                          )}
                        </div>
                      )}
                  </>
                )}
              </div>
            )}
          </div>

          {managementAllowed && showAddForm && !editingId && renderDatabaseForm(null)}
          </>
          )}

          {currentTab === 'catalogs' && <SeizaCatalogControls />}

          {currentTab === 'sync' && (
            <>
              <SchedulerSyncControls databases={databases} disabled={isApplying} />
              <RemoteSyncPreviews databases={databases} disabled={isApplying} />
              <RemotePeerSync databases={databases} disabled={isApplying} />
            </>
          )}

          {currentTab === 'setups' && (
            <>
              <ProcessingSetupsManager />
              <CalibrationMatchingSettings />
              <ExportDefaultsSettings />
            </>
          )}

          {currentTab === 'users' && (
            <UserManagement currentUsername={access.status.username} />
          )}
        </div>

        <div className="modal-footer">
          {statusMessage && (
            <div
              className={`save-message ${
                statusMessage.includes('Failed') || statusMessage.includes('failed')
                  ? 'error'
                  : 'success'
              }`}
            >
              {statusMessage}
            </div>
          )}
          <div className="modal-buttons">
            <button onClick={onClose} className="save-button" disabled={isApplying}>
              Done
            </button>
            <button
              onClick={handleRestart}
              className="cancel-button"
              disabled={isApplying}
              title={
                isTauri
                  ? 'Force a server restart (rarely needed — changes are applied live)'
                  : 'Reload the page'
              }
            >
              {isTauri ? 'Restart Server' : 'Reload Page'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
