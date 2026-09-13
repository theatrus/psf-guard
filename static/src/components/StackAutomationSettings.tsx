import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

/**
 * Server-wide automatic stack previews: whether remembered previews rebuild
 * on their own when frames arrive or grades change, and how long each kind of
 * change settles first. Persisted in the registry and applied at once.
 */
export default function StackAutomationSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['stack-automation-settings'],
    queryFn: apiClient.getStackAutomationSettings,
  });

  const [enabled, setEnabled] = useState(false);
  const [arrivalDraft, setArrivalDraft] = useState('');
  const [gradeDraft, setGradeDraft] = useState('');
  useEffect(() => {
    if (settings.data) {
      setEnabled(settings.data.automatic_previews);
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
  const parse = (draft: string) => (draft.trim() === '' ? NaN : Number(draft));
  const arrival = parse(arrivalDraft);
  const grade = parse(gradeDraft);
  const validMinutes = (minutes: number) =>
    Number.isInteger(minutes) && minutes >= 1 && minutes <= current.max_delay_minutes;
  const invalid = !validMinutes(arrival) || !validMinutes(grade);
  const dirty =
    enabled !== current.automatic_previews ||
    arrival !== current.arrival_delay_minutes ||
    grade !== current.grade_delay_minutes;

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
            checked={enabled}
            onChange={(event) => setEnabled(event.target.checked)}
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
              A night's stream of frames settles this long before a rebuild,
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
            aria-invalid={!validMinutes(arrival)}
            disabled={!enabled}
            onChange={(event) => setArrivalDraft(event.target.value)}
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
            aria-invalid={!validMinutes(grade)}
            disabled={!enabled}
            onChange={(event) => setGradeDraft(event.target.value)}
          />
        </label>
        {invalid && (
          <p className="error-text">
            Enter whole minutes between 1 and {current.max_delay_minutes}.
          </p>
        )}
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
      <button
        type="button"
        className="save-button"
        disabled={invalid || !dirty || save.isPending}
        onClick={() =>
          save.mutate({
            automatic_previews: enabled,
            arrival_delay_minutes: arrival,
            grade_delay_minutes: grade,
          })
        }
      >
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </div>
  );
}
