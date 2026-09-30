import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { useLocation } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import DirectorDashboard from '../director/DirectorDashboard';
import DirectorRigs from '../director/DirectorRigs';
import TemplateLibrary from '../director/TemplateLibrary';
import { useAccess } from '../../auth/access';
import { openSettings } from '../../utils/settingsIntent';
import '../director/DirectorPage.css';
import { liveSummary } from './liveSummary';
import { retryWhenBusy } from '../director/retry';
import './header.css';

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The fleet in the header: how many rigs, how many exposing, red when one
 *  has gone quiet. Opens the Live dashboard in a drawer from any view. It
 *  stays out of the header when Director is off or no rig exists yet. */
export default function LiveChip() {
  const director = useDirectorStatus();
  const enabled = !!director.data?.enabled && director.data.protocol_version === 1;
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, enabled, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000, refetchOnWindowFocus: true });
  const [open, setOpen] = useState(false);
  const { canWrite } = useAccess();
  const chipRef = useRef<HTMLButtonElement>(null);
  const drawerRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const location = useLocation();
  const close = () => { setOpen(false); chipRef.current?.focus(); };
  // A link inside the drawer (a plan's workspace) closes it on arrival.
  const [openedAt, setOpenedAt] = useState(location.key);
  if (open && location.key !== openedAt) { setOpen(false); setOpenedAt(location.key); }
  useEffect(() => { if (open) closeRef.current?.focus(); }, [open]);
  // Escape and Tab stay inside the drawer while it is open: it is modal.
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') { event.stopPropagation(); close(); return; }
    if (event.key !== 'Tab') return;
    const items = Array.from(drawerRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  };
  const rows = statuses.data ?? [];
  if (!enabled || rows.length === 0) return null;
  const summary = liveSummary(rows, Date.now());
  return <>
    <button ref={chipRef} type="button" className={`header-button utility-button live-chip${summary.alert ? ' is-alert' : ''}`} title={summary.title}
      aria-label={`Live rigs: ${summary.label}`} aria-haspopup="dialog" aria-expanded={open}
      onClick={() => { setOpenedAt(location.key); setOpen(current => !current); }}>
      <span className="live-chip-dot" aria-hidden="true" />
      <span className="utility-label">{summary.label}</span>
    </button>
    {open && <div className="live-drawer-backdrop" onClick={close}>
      <div ref={drawerRef} className="live-drawer" role="dialog" aria-modal="true" aria-label="Live rigs" onClick={event => event.stopPropagation()} onKeyDown={onKeyDown}>
        <button ref={closeRef} type="button" className="live-drawer-close" aria-label="Close live rigs" onClick={close}><X size={16} /></button>
        <div className="director-page director-embedded">
          <DirectorDashboard />
          {/* Editors set rigs and templates up in Settings; a read-only
              viewer cannot open Settings, so they read both here. */}
          {canWrite ? <p className="director-muted">
            Rig setup and the exposure template library are under Settings:{' '}
            <button type="button" className="director-link-button" onClick={() => { close(); openSettings('rigs'); }}>Rigs</button>
            {' and '}
            <button type="button" className="director-link-button" onClick={() => { close(); openSettings('templates'); }}>Exposure templates</button>.
          </p> : <><DirectorRigs /><TemplateLibrary /></>}
        </div>
      </div>
    </div>}
  </>;
}
