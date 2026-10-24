import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackAutomationSettings as Settings } from '../api/types';

/**
 * Server-wide automatic stack previews: whether remembered previews rebuild
 * on their own when frames arrive or grades change, how long each kind of
 * change settles first, and whether a refreshed stack gets its processing
 * again. Persisted in the registry and applied at once: a switch saves as
 * soon as it is flipped, a delay when you leave the field.
 */
export default function StackAutomationSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['stack-automation-settings'],
    queryFn: apiClient.getStackAutomationSettings,
  });

  // The delays edit as text so a half-typed value is not fought by the
  // parser; each commits when its field is left. A field without an edit
  // (null) shows the server's value, so a refetch updates it.
  const [arrivalDraft, setArrivalDraft] = useState<string | null>(null);
  const [gradeDraft, setGradeDraft] = useState<string | null>(null);
  // The new-channel window, in days. While any age is chosen the field shows
  // the days set aside when it was ticked, so unticking brings them back.
  const [windowDraft, setWindowDraft] = useState<string | null>(null);
  const [asideDays, setAsideDays] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (update: {
      automatic_previews: boolean;
      arrival_delay_minutes: number;
      grade_delay_minutes: number;
      build_new_channels: boolean;
      new_channel_window_days: number;
      reapply_processing: boolean;
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
  const arrivalText = arrivalDraft ?? String(current.arrival_delay_minutes);
  const gradeText = gradeDraft ?? String(current.grade_delay_minutes);
  const windowText =
    windowDraft ??
    (current.new_channel_window_days > 0
      ? String(current.new_channel_window_days)
      : asideDays ?? String(current.default_new_channel_window_days));
  const validMinutes = (draft: string) => {
    const minutes = draft.trim() === '' ? NaN : Number(draft);
    return Number.isInteger(minutes) && minutes >= 1 && minutes <= current.max_delay_minutes
      ? minutes
      : null;
  };
  const arrival = validMinutes(arrivalText);
  const grade = validMinutes(gradeText);
  const anyAge = current.new_channel_window_days === 0;
  // On unless the server says otherwise; an older server has no such
  // setting.
  const reapply = current.reapply_processing ?? true;
  const windowDays = (() => {
    const days = windowText.trim() === '' ? NaN : Number(windowText);
    return Number.isInteger(days) && days >= 1 && days <= 3650 ? days : null;
  })();

  const persist = (
    next: Partial<
      Pick<
        Settings,
        | 'automatic_previews'
        | 'arrival_delay_minutes'
        | 'grade_delay_minutes'
        | 'build_new_channels'
        | 'new_channel_window_days'
        | 'reapply_processing'
      >
    >,
    onSaved?: () => void
  ) =>
    save.mutate(
      {
        automatic_previews: current.automatic_previews,
        arrival_delay_minutes: current.arrival_delay_minutes,
        grade_delay_minutes: current.grade_delay_minutes,
        build_new_channels: current.build_new_channels,
        new_channel_window_days: current.new_channel_window_days,
        reapply_processing: reapply,
        ...next,
      },
      { onSuccess: onSaved }
    );
  // A field commits when it is left with a valid value that differs from the
  // saved one. An invalid value reverts to the saved one, and a failed save
  // keeps what was typed.
  const commitWindow = () => {
    if (anyAge || windowDays === null || windowDays === current.new_channel_window_days) setWindowDraft(null);
    else persist({ new_channel_window_days: windowDays }, () => setWindowDraft(null));
  };
  const commitArrival = () => {
    if (arrival === null || arrival === current.arrival_delay_minutes) setArrivalDraft(null);
    else persist({ arrival_delay_minutes: arrival }, () => setArrivalDraft(null));
  };
  const commitGrade = () => {
    if (grade === null || grade === current.grade_delay_minutes) setGradeDraft(null);
    else persist({ grade_delay_minutes: grade }, () => setGradeDraft(null));
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
            Automatically refresh stack previews
            <small>
              Refresh existing stack previews when frames arrive by import, upload or sync,
              or when grades change. PSF Guard rebuilds the same channels with the same settings.
              Automatic rebuilds pause while jobs you start run, then resume.
              This applies to every database on this server.
            </small>
          </span>
        </label>
        <label className="review-preference">
          <span>
            Wait after new frames (minutes)
            <small>
              Wait this long after the last new frame before rebuilding. Each new frame
              restarts the timer, so PSF Guard processes frames in batches. Default{' '}
              {current.default_arrival_delay_minutes}.
            </small>
          </span>
          <input
            type="number"
            min={1}
            max={current.max_delay_minutes}
            step={1}
            value={arrivalText}
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
              Wait this long after the last grade change before rebuilding.
              Each further change restarts the timer. Default{' '}
              {current.default_grade_delay_minutes}.
            </small>
          </span>
          <input
            type="number"
            min={1}
            max={current.max_delay_minutes}
            step={1}
            value={gradeText}
            aria-label="Minutes to wait after grade changes"
            aria-invalid={grade === null}
            disabled={!current.automatic_previews}
            onChange={(event) => setGradeDraft(event.target.value)}
            onBlur={commitGrade}
            onKeyDown={commitOnEnter(commitGrade)}
          />
        </label>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={current.build_new_channels}
            disabled={!current.automatic_previews}
            onChange={(event) => persist({ build_new_channels: event.target.checked })}
          />
          <span>
            Also stack channels that have no stack yet
            <small>
              Frames that arrive are stacked before you open the project: a
              channel with two or more usable frames, one of them recent
              enough, is stacked with the project's settings.
            </small>
          </span>
        </label>
        <label className="review-preference">
          <span>
            Recent-frame requirement (days)
            <small>
              Build a new channel only if it has a frame captured within this many days.
              Older channels are excluded. Default{' '}
              {current.default_new_channel_window_days}.
            </small>
          </span>
          <input
            type="number"
            min={1}
            max={3650}
            step={1}
            value={windowText}
            aria-label="Recent-frame requirement (days)"
            aria-invalid={!anyAge && windowDays === null}
            disabled={!current.automatic_previews || !current.build_new_channels || anyAge}
            onChange={(event) => setWindowDraft(event.target.value)}
            onBlur={commitWindow}
            onKeyDown={commitOnEnter(commitWindow)}
          />
        </label>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={anyAge}
            disabled={!current.automatic_previews || !current.build_new_channels}
            onChange={(event) => {
              if (event.target.checked) {
                setAsideDays(windowText);
                persist({ new_channel_window_days: 0 });
              } else {
                persist({ new_channel_window_days: windowDays ?? current.default_new_channel_window_days });
              }
            }}
          />
          <span>
            Also stack old channels, of any age
            <small>
              At the next refresh, build all eligible channels without a stack, regardless
              of frame age. This may take a long time for a large catalog.
              Background jobs pause while jobs you start run.
            </small>
          </span>
        </label>
        {(arrival === null || grade === null) && (
          <p className="error-text">
            Enter whole minutes between 1 and {current.max_delay_minutes}.
          </p>
        )}
      </fieldset>
      <fieldset
        className="calibration-settings-group calibration-matching-group"
        disabled={save.isPending}
      >
        <legend>Processing</legend>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={reapply}
            onChange={(event) => persist({ reapply_processing: event.target.checked })}
          />
          <span>
            Re-apply processing when a stack refreshes
            <small>
              After a stack rebuild, PSF Guard reapplies its stretch, deconvolution and
              RC-Astro settings in the background. Jobs you start take priority.
              The card shows the previous result until the new one is ready.
              Turning this off cancels processing jobs that have not started.
            </small>
          </span>
        </label>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}
