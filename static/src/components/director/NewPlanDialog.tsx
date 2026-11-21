import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Check } from 'lucide-react';
import { apiClient } from '../../api/client';
import Dialog from '../Dialog';
import { identityId } from './identityId';
import { planHref } from './planAddress';
import { NEW_PLAN_STATE, planNameProblem } from './planName';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** Start a plan from nothing: give it a name, and PlanPage opens it on
 *  Framing. Trying again under the same name reuses the plan's id, so a
 *  create whose answer was lost cannot make a second plan. A new name gets
 *  a new id, since the server refuses an id it holds under another name. */
export default function NewPlanDialog({ onClose }: { onClose: () => void }) {
  const client = useQueryClient();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const attempt = useRef<{ id: string; name: string } | null>(null);
  const [name, setName] = useState('');
  const [validation, setValidation] = useState('');
  const input = useRef<HTMLInputElement>(null);
  // Dialog focuses its close button as it opens; the name comes first here.
  useEffect(() => { const handle = setTimeout(() => input.current?.focus(), 0); return () => clearTimeout(handle); }, []);
  const create = useMutation({
    retry: false,
    mutationFn: async (trimmed: string) => {
      const id = attempt.current?.name === trimmed ? attempt.current.id : identityId();
      attempt.current = { id, name: trimmed };
      await apiClient.createDirectorIdentity('projects', { id, name: trimmed });
      return id;
    },
    onSuccess: async id => {
      // The plan page finds the plan in this list, so it must hold it first.
      await client.invalidateQueries({ queryKey: ['directorPlans'] });
      onClose();
      navigate(planHref(id, params), { state: NEW_PLAN_STATE });
    },
  });
  const busy = create.isPending;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    const trimmed = name.trim();
    const problem = planNameProblem(trimmed);
    setValidation(problem);
    if (!problem) create.mutate(trimmed);
  };
  return (
    <Dialog open title="New plan" className="new-plan-dialog" onClose={() => { if (!busy) onClose(); }}
      footer={<>
        <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
        <button type="submit" form="new-plan-form" className="action-button primary" disabled={busy || !name.trim()}>
          <Check size={16} />{busy ? 'Creating...' : 'Create plan'}
        </button>
      </>}>
      <form id="new-plan-form" className="director-edit" onSubmit={submit}>
        <p className="director-muted">The plan opens on Framing, where you choose its target.</p>
        <label htmlFor="new-plan-name">Plan name</label>
        <input id="new-plan-name" ref={input} value={name} onChange={event => setName(event.target.value)} required maxLength={512} disabled={busy} />
        {(validation || create.isError) && <p className="director-error" role="alert">{validation || message(create.error)}</p>}
      </form>
    </Dialog>
  );
}
