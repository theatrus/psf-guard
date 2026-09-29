import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Search } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorNameSearchHit, DirectorSkyPosition } from '../../api/directorTypes';
import { useDebounced } from './useSurveyCutout';
import './TargetSearch.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Lookup failed';
const compact = (value: string) => value.replace(/[^\p{L}\p{N}]/gu, '').toUpperCase();
const KIND_LABEL: Record<string, string> = {
  galaxy: 'galaxy', 'open-cluster': 'open cluster', 'globular-cluster': 'globular cluster', nebula: 'nebula', 'planetary-nebula': 'planetary nebula',
  'hii-region': 'H II region', 'supernova-remnant': 'supernova remnant', 'dark-nebula': 'dark nebula', 'cluster-nebula': 'cluster and nebula',
  star: 'star', 'double-star': 'double star', association: 'association', transient: 'transient',
};

export interface TargetPick { name: string; center: DirectorSkyPosition; source: string }

/** A name box with a dropdown: the local Seiza catalog answers as you type,
 *  and the last row asks CDS Sesame for the name as typed (the server keeps
 *  those answers). Enter takes the highlighted row, else an exact local
 *  match, else goes online. */
export default function TargetSearch({ label, ariaLabel, placeholder, buttonLabel, disabled = false, hint, onPick }: {
  label: string; ariaLabel: string; placeholder?: string; buttonLabel: string; disabled?: boolean; hint: string; onPick: (pick: TargetPick) => void;
}) {
  const [text, setText] = useState('');
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [picked, setPicked] = useState<TargetPick | null>(null);
  const typed = useDebounced(text.trim(), 150);
  const listId = useId();
  const input = useRef<HTMLInputElement>(null);
  const suggestions = useQuery({
    queryKey: ['directorNameSearch', typed],
    queryFn: () => apiClient.searchDirectorNames(typed),
    enabled: typed.length >= 2, staleTime: 5 * 60_000, retry: false, refetchOnWindowFocus: false, placeholderData: previous => previous,
  });
  const local: DirectorNameSearchHit[] = useMemo(() => typed.length >= 2 && suggestions.data ? suggestions.data.local.items : [], [typed, suggestions.data]);
  const online = useMutation({
    retry: false,
    mutationFn: (query: string) => apiClient.resolveDirectorName(query),
    onSuccess: hit => choose({ name: hit.name, center: { ra_degrees: hit.ra_degrees, dec_degrees: hit.dec_degrees }, source: hit.source }),
  });
  const choose = (pick: TargetPick) => { setPicked(pick); setOpen(false); setActive(-1); onPick(pick); };
  const pickLocal = (hit: DirectorNameSearchHit) => choose({ name: hit.name, center: { ra_degrees: hit.ra_degrees, dec_degrees: hit.dec_degrees }, source: hit.source });
  const query = text.trim();
  const rows = local.length + (query ? 1 : 0);
  useEffect(() => { if (active >= rows) setActive(rows - 1); }, [rows, active]);
  const go = () => {
    if (!query || disabled || online.isPending) return;
    if (active >= 0 && active < local.length) { pickLocal(local[active]); return; }
    if (active < local.length) {
      const exact = local.find(hit => compact(hit.matched) === compact(query) || compact(hit.name) === compact(query) || compact(hit.common_name) === compact(query));
      if (exact) { pickLocal(exact); return; }
    }
    setOpen(false);
    online.mutate(query);
  };
  const status = online.isPending ? 'Looking up...'
    : online.isError ? message(online.error)
    : picked ? `${picked.name} from ${picked.source}; the target and view moved there.`
    : hint;
  return <label className="target-search">{label}
    <span className="framing-input">
      <span className="target-search-box">
        <input ref={input} aria-label={ariaLabel} role="combobox" aria-expanded={open && rows > 0} aria-controls={listId} aria-autocomplete="list"
          aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
          value={text} maxLength={128} placeholder={placeholder} disabled={disabled || online.isPending} autoComplete="off"
          onChange={event => { setText(event.target.value); setOpen(true); setActive(-1); online.reset(); }}
          onFocus={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={event => {
            if (event.key === 'ArrowDown' && rows > 0) { event.preventDefault(); setOpen(true); setActive(current => (current + 1) % rows); }
            else if (event.key === 'ArrowUp' && rows > 0) { event.preventDefault(); setOpen(true); setActive(current => (current <= 0 ? rows - 1 : current - 1)); }
            else if (event.key === 'Escape') { setOpen(false); setActive(-1); }
            else if (event.key === 'Enter') { event.preventDefault(); go(); }
          }} />
        {open && rows > 0 && !disabled && <ul className="target-search-list" role="listbox" id={listId} aria-label="Matching names" onMouseDown={event => event.preventDefault()}>
          {local.map((hit, index) => <li key={hit.name} id={`${listId}-${index}`} role="option" aria-selected={index === active} className={index === active ? 'is-active' : undefined}
            onMouseEnter={() => setActive(index)} onClick={() => pickLocal(hit)}>
            <span className="target-search-name">{hit.name}</span>
            {hit.common_name && <span className="target-search-common">{hit.common_name}</span>}
            <span className="target-search-kind">{KIND_LABEL[hit.kind] ?? hit.kind}</span>
          </li>)}
          {query && <li id={`${listId}-${local.length}`} role="option" aria-selected={active === local.length} className={`target-search-online${active === local.length ? ' is-active' : ''}`}
            onMouseEnter={() => setActive(local.length)} onClick={() => { setOpen(false); online.mutate(query); }}>
            <Search size={13} />Look up “{query}” online{suggestions.data && !suggestions.data.local.available ? ' (no local catalog on this server)' : ''}
          </li>}
        </ul>}
      </span>
      <button type="button" disabled={disabled || !query || online.isPending} onClick={go}><Search size={16} />{online.isPending ? 'Looking up...' : buttonLabel}</button>
    </span>
    <small role={online.isError ? 'alert' : undefined}>{status}</small>
  </label>;
}
