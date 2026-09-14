import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackAutomationSettings as Settings } from '../api/types';

/**
 * Server-wide automatic stack previews: whether remembered previews rebuild
 * on their own when frames arrive or grades change, and how long each kind of
 * change settles first. Persisted in the registry and applied at once: the
 * switch saves as soon as it is flipped, a delay when you leave the field.
 */
export default function StackAutomationSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['stack-automation-settings'],
    queryFn: apiClient.getStackAutomationSettings,
  });

  // The delays edit as text so a half-typed value is not fought by the
  // parser; each commits when its field is left.
  const [arrivalDraft, setArrivalDraft] = useState('');
  const [gradeDraft, setGradeDraft] = useState('');
  useEffect(() => {
    if (settings.data) {
      setArrivalDraft(String(settings.data.arrival_delay_minutes));
      setGradeDraft(String(settings.data.grade_delay_minutes));
    }
  }, [settings.data]);

  const save = useMutation({
    mutationFn: (update: {
      automatic_previews: boolean;
      arrival_delay_minutes: number;
      grade_delay_minutes: number;
    }) => apiClient.updateStackAutomationSettings(update),
    onSuccess: (updated) => {
      queryClient.setQueryData(['stack-automation-settings'], updated);
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="stack-automation-settings">
        <h3>Stack previews</h3>
        <p className="muted" role="alert">Could not load stack preview settings.</p>
      </div>
    );
  }

  const current = settings.data!;
  const validMinutes = (draft: string) => {
    const minutes = draft.trim() === '' ? NaN : Number(draft);
    return Number.isInteger(minutes) && minutes >= 1 && minutes <= current.max_delay_minutes
      ? minutes
      : null;
  };
  const arrival = validMinutes(arrivalDraft);
  const grade = validMinutes(gradeDraft);

  const persist = (next: Partial<Pick<Settings, 'automatic_previews' | 'arrival_delay_minutes' | 'grade_delay_minutes'>>) =>
    save.mutate({
      automatic_previews: current.automatic_previews,
      arrival_delay_minutes: current.arrival_delay_minutes,
      grade_delay_minutes: current.grade_delay_minutes,
      ...next,
    });
  // A delay commits when its field is left with a valid value that differs
  // from what the server holds; an invalid one snaps back.
  const commitArrival = () => {
    if (arrival === null) setArrivalDraft(String(current.arrival_delay_minutes));
    else if (arrival !== current.arrival_delay_minutes) persist({ arrival_delay_minutes: arrival });
  };
  const commitGrade = () => {
    if (grade === null) setGradeDraft(String(current.grade_delay_minutes));
    else if (grade !== current.grade_delay_minutes) persist({ grade_delay_minutes: grade });
  };
  const commitOnEnter = (commit: () => void) => (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter') {
      commit();
      event.currentTarget.blur();
    }
  };

  return (
    <div className="stack-automation-settings">
      <h3>Stack previews</h3>
      <fieldset
        className="calibration-settings-group calibration-matching-group"
        disabled={save.isPending}
      >
        <legend>Automatic refresh</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={current.automatic_previews}
            onChange={(event) => persist({ automatic_previews: event.target.checked })}
          />
          <span>
            Rebuild stack previews on their own
            <small>
              A project whose previews were built once keeps them current:
              frames that arrive by import, upload, or sync, and grades that
              change, queue a rebuild of the same channels with the same
              settings. A build you start takes precedence; a rebuild steps
              aside and comes back afterwards. Applies to every database on
              this server.
            </small>
          </span>
        </label>
        <label className="review-preference">
          <span>
            Wait after new frames (minutes)
            <small>
              A night&apos;s stream of frames settles this long before a rebuild,
              so it runs in batches rather than after every frame. Default{' '}
              {current.default_arrival_delay_minutes}.
            </small>
          </span>
          <input
            type="number"
            min={1}
            max={current.max_delay_minutes}
            step={1}
            value={arrivalDraft}
            aria-label="Minutes to wait after new frames"
            aria-invalid={arrival === null}
            disabled={!current.automatic_previews}
            onChange={(event) => setArrivalDraft(event.target.value)}
            onBlur={commitArrival}
            onKeyDown={commitOnEnter(commitArrival)}
          />
        </label>
        <label className="review-preference">
          <span>
            Wait after grade changes (minutes)
            <small>
              Grading is interactive, so a change waits longer, and each
              further change pushes the rebuild out again. Default{' '}
              {current.default_grade_delay_minutes}.
            </small>
          </span>
          <input
            type="number"
            min={1}
            max={current.max_delay_minutes}
            step={1}
            value={gradeDraft}
            aria-label="Minutes to wait after grade changes"
            aria-invalid={grade === null}
            disabled={!current.automatic_previews}
            onChange={(event) => setGradeDraft(event.target.value)}
            onBlur={commitGrade}
            onKeyDown={commitOnEnter(commitGrade)}
          />
        </label>
        {(arrival === null || grade === null) && (
          <p className="error-text">
            Enter whole minutes between 1 and {current.max_delay_minutes}.
          </p>
        )}
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
