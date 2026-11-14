import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ImportedCopiesReport, RenamedLight } from '../api/types';

const EXAMPLES = 5;

function plural(count: number, one: string, many: string): string {
  return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function targetOf(light: RenamedLight): string {
  return light.target ?? 'no target';
}

function names(items: string[]): string {
  const shown = items.slice(0, EXAMPLES).join(', ');
  return items.length > EXAMPLES ? `${shown} and ${plural(items.length - EXAMPLES, 'other', 'others')}` : shown;
}

/** Lights an import catalogued beside Target Scheduler's records of the same
 *  files, usually after merging two catalogs, and the empty projects and
 *  targets the import made. Silent when there is nothing to say. */
export default function ImportedCopiesRepair({ dbId, dbName, canManage }: { dbId: string; dbName: string; canManage: boolean }) {
  const client = useQueryClient();
  const key = ['db', dbId, 'imported-copies'];
  const [changed, setChanged] = useState(false);
  const report = useQuery({ queryKey: key, queryFn: () => apiClient.getImportedCopies(dbId), staleTime: 60_000, retry: false });
  const merge = useMutation({
    retry: false,
    mutationFn: (digest: string) => apiClient.mergeImportedCopies(dbId, digest),
    onSuccess: (outcome) => {
      if (outcome.state === 'changed') {
        setChanged(true);
        client.setQueryData<ImportedCopiesReport>(key, outcome.preview);
        return;
      }
      void client.invalidateQueries({ queryKey: ['db', dbId] });
    },
  });
  const done = merge.data?.state === 'merged' ? merge.data : null;
  if (done) {
    const removed = [
      done.projects.length > 0 && plural(done.projects.length, 'empty project', 'empty projects'),
      done.targets.length > 0 && plural(done.targets.length, 'empty target', 'empty targets'),
    ].filter(Boolean).join(' and ');
    return <div className="settings-note" role="status">
      {done.backup_path
        ? <>Merged {plural(done.groups.length, 'light', 'lights')} with imported copies in {dbName}{removed && <>, and deleted {removed}</>}. The database was copied first to <code>{done.backup_path}</code>.</>
        : <>{dbName} has no imported copies to merge; nothing was changed.</>}
    </div>;
  }
  const data = report.data;
  if (!data) return null;
  const { groups, held, projects, targets, kept_projects: keptProjects } = data;
  if (groups.length + held.length + projects.length + targets.length === 0) return null;
  const actionable = groups.length + projects.length + targets.length > 0;
  return <div className="settings-note settings-warning" role="note" aria-label={`Imported copies in ${dbName}`}>
    {groups.length + held.length > 0
      ? <p>
          <strong>{plural(groups.length + held.length, 'light is', 'lights are')} in the catalog twice:</strong>{' '}
          as Target Scheduler recorded {groups.length + held.length === 1 ? 'it' : 'them'}, and from an import of the same file.
        </p>
      : <p><strong>An earlier merge left projects or targets an import created with no lights.</strong></p>}
    {groups.length > 0 && <ul>
      {groups.slice(0, EXAMPLES).map(group => <li key={group.kept.id}>
        <code>{baseName(group.kept.file_name)}</code> in {targetOf(group.kept)}, imported again in {[...new Set(group.copies.map(targetOf))].join(', ')}
      </li>)}
      {groups.length > EXAMPLES && <li>and {plural(groups.length - EXAMPLES, 'other', 'others')}</li>}
    </ul>}
    {held.length > 0 && <p>
      {plural(held.length, 'light stays', 'lights stay')} as {held.length === 1 ? 'it is' : 'they are'}:{' '}
      {held.slice(0, EXAMPLES).map(group => `${baseName(group.kept.file_name)} (${group.reason})`).join('; ')}
      {held.length > EXAMPLES ? `; and ${plural(held.length - EXAMPLES, 'other', 'others')}` : ''}.
    </p>}
    {projects.length > 0 && <p>
      The merge deletes {projects.length === 1 ? 'this project' : 'these projects'}, which an import created and which will have no lights:{' '}
      {names(projects.map(project => project.targets.length > 1 ? `${project.name} (${project.targets.length} targets)` : project.name))}.
    </p>}
    {targets.length > 0 && <p>
      It also deletes {targets.length === 1 ? 'this empty target' : 'these empty targets'}:{' '}
      {names(targets.map(target => `${target.name} in ${target.project}`))}.
    </p>}
    {keptProjects.length > 0 && <p>
      {plural(keptProjects.length, 'empty imported project stays', 'empty imported projects stay')}:{' '}
      {keptProjects.slice(0, EXAMPLES).map(project => `${project.name} (${project.reason})`).join('; ')}.
    </p>}
    {changed && <p role="status">{dbName} changed since this list was made, so nothing was merged. Check the list above, then merge again.</p>}
    {actionable && (canManage && data.writable
      ? <p>
          <button type="button" className="browse-button" disabled={merge.isPending} onClick={() => { setChanged(false); merge.mutate(data.digest); }}>
            {merge.isPending ? 'Merging…' : 'Merge imported copies'}
          </button>{' '}
          Copies the database beside itself first. Target Scheduler&apos;s light keeps its project, target and exposure plan,
          and takes the copy&apos;s grade when it has none. The copies are removed, and a sync does not bring them back.
          Close N.I.N.A. on that rig before merging.
        </p>
      : <p>
          {canManage ? 'This server cannot write the file. ' : 'An editor can merge them, or '}
          Run <code>psf-guard merge-imported-copies --db {dbId} --apply</code> where the file is writable.
        </p>)}
    {merge.isError && <p className="settings-error" role="alert">{merge.error instanceof Error ? merge.error.message : 'The merge failed.'}</p>}
  </div>;
}
