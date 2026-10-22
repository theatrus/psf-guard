import { useMemo, useState } from 'react';
import type {
  StackBackgroundConfig,
  StackBackgroundExtraction,
  StackBackgroundFit,
  StackBackgroundProtection,
  StackColorProcessing,
  StackColorJob,
  StackColorRole,
  StackDeconvolutionResult,
  StackStretchRequest,
  ExternalToolSchema,
  RcAstroParameterValue,
} from '../api/types';
import StackDeconvolutionControls, { StackDeconvolutionSummary } from './StackDeconvolutionControls';
import StackRcAstroControls from './StackRcAstroControls';
import {
  bxtCorrectOnly,
  defaultParameters,
  offersCorrectOnly,
  rcAstroProblem,
  useRcAstroCapabilities,
} from './rcAstroSchema';
import { validateDeconvolution } from './stackDeconvolution';
import StackStretchStageEditor from './StackStretchStageEditor';
import { defaultStretchRequest, stretchModelLabels } from './stackStretchModels';
import {
  colorCalibrationApplies,
  defaultBackgroundModel,
  defaultBackgroundExtraction,
  defaultColorProcessing,
} from './stackColorProcessing';
import ColorCalibrationSwitch from './ColorCalibrationSwitch';
import ProcessingSetupsBar from './ProcessingSetupsBar';
import { builtinColorSetups, colorSetupForRoles } from './processingSetups';
import {
  channelToolsLabel,
  ownFromChannels,
  type ChannelPixelProcessing,
  type ChannelProcessing,
} from './channelProcessing';
import './toolbar/toolbar.css';

const roleLabels: Record<StackColorRole, string> = {
  luminance: 'L', red: 'R', green: 'G', blue: 'B', ha: 'Hα', oiii: 'OIII', sii: 'SII',
};

type AutomaticBackgroundModel = Extract<
  StackBackgroundConfig['model'], { kind: 'automatic' }
>;
type PolynomialBackgroundModel = Extract<
  StackBackgroundConfig['model'], { kind: 'polynomial' }
>;
type RadialBasisBackgroundModel = Extract<
  StackBackgroundConfig['model'], { kind: 'radial_basis' }
>;

function cloneProcessing(processing: StackColorProcessing): StackColorProcessing {
  return JSON.parse(JSON.stringify(processing)) as StackColorProcessing;
}

function hasInvalidNumbers(processing: StackColorProcessing): boolean {
  const stages = [
    ...Object.values(processing.input_stretches).flatMap((value) => value ?? []),
    ...processing.output_stretches,
  ];
  return stages.some((stage) => Object.values(stage.model).some(
    (value) => typeof value === 'number' && !Number.isFinite(value)
  )) || Object.values(processing.input_deconvolutions).some((config) =>
    config && Object.values(config).some((value) => !Number.isFinite(value))
  ) || [...Object.values(processing.input_rc_astro ?? {}), processing.color_rc_astro]
    .some((config) =>
      config?.steps.some((step) => Object.values(step.parameters).some((value) =>
        typeof value === 'number' && !Number.isFinite(value)
      ))
    );
}

/** The tools that run on the color channels together: star removal runs on
 *  the channel stacks only. */
const colorChannelTools = ['bxt', 'nxt'] as const;

/** On the color channels, BlurXTerminator starts in Correct only mode when
 *  the installed tool can run it: it fixes star shapes and color fringes,
 *  and each channel's own run can sharpen. */
function colorChannelDefaults(
  schema: ExternalToolSchema
): Record<string, RcAstroParameterValue> {
  return offersCorrectOnly(schema) ? { ...bxtCorrectOnly } : defaultParameters(schema);
}

/** JSON with object keys sorted, so a draft compares equal to what the
 *  server applied whatever order its edits left the keys in. */
function canonicalJson(value: unknown): string {
  return JSON.stringify(value, (_key, inner: unknown) =>
    inner && typeof inner === 'object' && !Array.isArray(inner)
      ? Object.fromEntries(Object.entries(inner).sort(([left], [right]) => left.localeCompare(right)))
      : inner
  );
}

function validateBackground(extraction: StackBackgroundExtraction | null): string | null {
  if (!extraction) return null;
  const { config } = extraction;
  if (!Number.isFinite(extraction.strength) || extraction.strength < 0 || extraction.strength > 1) {
    return 'Background correction strength must be between 0 and 1';
  }
  if (config.model.kind === 'polynomial') {
    if (!Number.isInteger(config.model.degree) || config.model.degree < 0 || config.model.degree > 4) {
      return 'Background polynomial degree must be an integer from 0 to 4';
    }
    if (!Number.isFinite(config.model.ridge) || config.model.ridge < 0) {
      return 'Background ridge must be finite and non-negative';
    }
  } else if (config.model.kind === 'radial_basis') {
    if (!Number.isFinite(config.model.smoothing) || config.model.smoothing < 0 ||
        !Number.isInteger(config.model.max_control_points) ||
        config.model.max_control_points < 16 || config.model.max_control_points > 512) {
      return 'Radial-basis smoothing must be non-negative and control points must be 16 to 512';
    }
  } else {
    if (!Number.isInteger(config.model.max_degree) ||
        config.model.max_degree < 0 || config.model.max_degree > 4 ||
        !Number.isFinite(config.model.ridge) || config.model.ridge < 0 ||
        !Number.isFinite(config.model.rbf_smoothing) || config.model.rbf_smoothing < 0 ||
        !Number.isInteger(config.model.max_control_points) ||
        config.model.max_control_points < 16 || config.model.max_control_points > 512 ||
        !Number.isFinite(config.model.minimum_improvement) ||
        config.model.minimum_improvement < 0 || config.model.minimum_improvement > 0.75) {
      return 'Automatic background model settings are outside their supported ranges';
    }
  }
  if (!Number.isInteger(config.samples_per_axis) ||
      config.samples_per_axis < 3 || config.samples_per_axis > 512) {
    return 'Background samples per axis must be an integer from 3 to 512';
  }
  if (config.sample_radius !== null &&
      (!Number.isInteger(config.sample_radius) || config.sample_radius < 1)) {
    return 'Background sample radius must be blank for automatic or a positive integer';
  }
  if (!Number.isInteger(config.search_steps) || config.search_steps < 0 ||
      config.search_steps > 64) {
    return 'Background search steps must be an integer from 0 to 64';
  }
  if (!Number.isFinite(config.sample_rejection_sigma) || config.sample_rejection_sigma <= 0 ||
      !Number.isFinite(config.fit_rejection_sigma) || config.fit_rejection_sigma <= 0) {
    return 'Background rejection sigmas must be finite and greater than zero';
  }
  if (!Number.isInteger(config.fit_rejection_iterations) ||
      config.fit_rejection_iterations < 0 || config.fit_rejection_iterations > 16) {
    return 'Background rejection passes must be an integer from 0 to 16';
  }
  if (!Number.isFinite(config.border_fraction) ||
      config.border_fraction < 0 || config.border_fraction >= 0.45) {
    return 'Background border fraction must be in the range 0 to 0.45';
  }
  return null;
}

function BackgroundNumberField({
  label, value, min, max, step = 'any', disabled, onChange,
}: {
  label: string;
  value: number | null;
  min?: number;
  max?: number;
  step?: number | 'any';
  disabled: boolean;
  onChange: (value: number | null) => void;
}) {
  return (
    <label className="stack-stretch-field">
      <span>{label}</span>
      <input
        type="number"
        aria-label={`Background ${label}`}
        value={value ?? ''}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        placeholder={value === null ? 'Auto' : undefined}
        onChange={(event) => onChange(
          event.target.value === '' ? null : event.target.valueAsNumber
        )}
      />
    </label>
  );
}

function BackgroundControls({
  extraction, backgrounds, protections, fallbacks, disabled, onChange,
}: {
  extraction: StackBackgroundExtraction | null;
  backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  protections: Partial<Record<StackColorRole, StackBackgroundProtection>>;
  fallbacks: Partial<Record<StackColorRole, string>>;
  disabled: boolean;
  onChange: (extraction: StackBackgroundExtraction | null) => void;
}) {
  const updateConfig = (change: Partial<StackBackgroundConfig>) => {
    if (!extraction) return;
    onChange({ ...extraction, config: { ...extraction.config, ...change } });
  };
  const updateModel = (model: StackBackgroundConfig['model']) => {
    if (!extraction) return;
    updateConfig({ model });
  };
  const selectModel = (kind: StackBackgroundConfig['model']['kind']) => {
    updateModel(defaultBackgroundModel(kind));
  };
  const updateAutomaticModel = (change: Partial<AutomaticBackgroundModel>) => {
    const model = extraction?.config.model;
    if (model?.kind === 'automatic') updateModel({ ...model, ...change });
  };
  const updatePolynomialModel = (change: Partial<PolynomialBackgroundModel>) => {
    const model = extraction?.config.model;
    if (model?.kind === 'polynomial') updateModel({ ...model, ...change });
  };
  const updateRadialBasisModel = (change: Partial<RadialBasisBackgroundModel>) => {
    const model = extraction?.config.model;
    if (model?.kind === 'radial_basis') updateModel({ ...model, ...change });
  };
  const fits = Object.entries(backgrounds) as Array<[StackColorRole, StackBackgroundFit]>;

  return (
    <section className="stack-background-controls" aria-label="Background extraction">
      <header>
        <label>
          <input
            type="checkbox"
            checked={extraction !== null}
            disabled={disabled}
            onChange={(event) => onChange(
              event.target.checked ? defaultBackgroundExtraction() : null
            )}
          />
          <strong>Background extraction</strong>
        </label>
        <span>{extraction ? 'Before registration' : 'Disabled'}</span>
      </header>
      <p>
        Fit and correct each linear channel independently before alignment. Subtraction removes
        additive gradients while preserving that channel&apos;s robust sky level.
      </p>
      {extraction && (
        <div className="stack-background-fields">
          <label className="stack-stretch-field">
            <span>Correction</span>
            <select
              aria-label="Background correction mode"
              value={extraction.correction_mode}
              disabled={disabled}
              onChange={(event) => onChange({
                ...extraction,
                correction_mode: event.target.value as 'subtract' | 'divide',
              })}
            >
              <option value="subtract">Subtract</option>
              <option value="divide">Divide</option>
            </select>
          </label>
          <BackgroundNumberField label="Strength" value={extraction.strength}
            min={0} max={1} step={0.05} disabled={disabled}
            onChange={(strength) => onChange({ ...extraction, strength: strength ?? 0 })} />
          <label className="stack-stretch-field stack-background-toggle">
            <span>Protect catalog emission</span>
            <input type="checkbox" aria-label="Protect catalog emission from background fitting"
              checked={extraction.protect_catalog_emission ?? true} disabled={disabled}
              onChange={(event) => onChange({
                ...extraction,
                protect_catalog_emission: event.target.checked,
              })} />
          </label>
          <label className="stack-stretch-field">
            <span>Surface model</span>
            <select
              aria-label="Background surface model"
              value={extraction.config.model.kind}
              disabled={disabled}
              onChange={(event) => selectModel(
                event.target.value as StackBackgroundConfig['model']['kind']
              )}
            >
              <option value="automatic">Automatic</option>
              <option value="polynomial">Polynomial</option>
              <option value="radial_basis">Radial basis (advanced)</option>
            </select>
          </label>
          {extraction.config.model.kind === 'automatic' && (
            <>
              <BackgroundNumberField label="Maximum degree"
                value={extraction.config.model.max_degree}
                min={0} max={4} step={1} disabled={disabled}
                onChange={(max_degree) => updateAutomaticModel({ max_degree: max_degree ?? 0 })} />
              <BackgroundNumberField label="Minimum improvement"
                value={extraction.config.model.minimum_improvement}
                min={0} max={0.75} step={0.01} disabled={disabled}
                onChange={(minimum_improvement) => updateAutomaticModel({
                  minimum_improvement: minimum_improvement ?? 0,
                })} />
              <label className="stack-stretch-field stack-background-toggle">
                <span>Flexible surface</span>
                <input type="checkbox" aria-label="Allow radial-basis background model"
                  checked={extraction.config.model.allow_radial_basis} disabled={disabled}
                  onChange={(event) => updateAutomaticModel({
                    allow_radial_basis: event.target.checked,
                  })} />
              </label>
              {extraction.config.model.allow_radial_basis && (
                <>
                  <BackgroundNumberField label="RBF smoothing"
                    value={extraction.config.model.rbf_smoothing}
                    min={0} step="any" disabled={disabled}
                    onChange={(rbf_smoothing) => updateAutomaticModel({
                      rbf_smoothing: rbf_smoothing ?? 0,
                    })} />
                  <BackgroundNumberField label="RBF control points"
                    value={extraction.config.model.max_control_points}
                    min={16} max={512} step={1} disabled={disabled}
                    onChange={(max_control_points) => updateAutomaticModel({
                      max_control_points: max_control_points ?? 16,
                    })} />
                </>
              )}
            </>
          )}
          {extraction.config.model.kind === 'polynomial' && (
            <BackgroundNumberField label="Polynomial degree"
              value={extraction.config.model.degree}
              min={0} max={4} step={1} disabled={disabled}
              onChange={(degree) => updatePolynomialModel({ degree: degree ?? 0 })} />
          )}
          {extraction.config.model.kind === 'radial_basis' && (
            <>
              <BackgroundNumberField label="RBF smoothing"
                value={extraction.config.model.smoothing}
                min={0} step="any" disabled={disabled}
                onChange={(smoothing) => updateRadialBasisModel({ smoothing: smoothing ?? 0 })} />
              <BackgroundNumberField label="RBF control points"
                value={extraction.config.model.max_control_points}
                min={16} max={512} step={1} disabled={disabled}
                onChange={(max_control_points) => updateRadialBasisModel({
                  max_control_points: max_control_points ?? 16,
                })} />
            </>
          )}
          <BackgroundNumberField label="Samples per axis" value={extraction.config.samples_per_axis}
            min={3} max={512} step={1} disabled={disabled}
            onChange={(samples_per_axis) => updateConfig({ samples_per_axis: samples_per_axis ?? 3 })} />
          <BackgroundNumberField label="Sample radius" value={extraction.config.sample_radius}
            min={1} step={1} disabled={disabled}
            onChange={(sample_radius) => updateConfig({ sample_radius })} />
          <BackgroundNumberField label="Search steps" value={extraction.config.search_steps}
            min={0} max={64} step={1} disabled={disabled}
            onChange={(search_steps) => updateConfig({ search_steps: search_steps ?? 0 })} />
          <BackgroundNumberField label="Noise sigma" value={extraction.config.sample_rejection_sigma}
            min={0.1} step={0.1} disabled={disabled}
            onChange={(sample_rejection_sigma) => updateConfig({
              sample_rejection_sigma: sample_rejection_sigma ?? 0,
            })} />
          <BackgroundNumberField label="Fit sigma" value={extraction.config.fit_rejection_sigma}
            min={0.1} step={0.1} disabled={disabled}
            onChange={(fit_rejection_sigma) => updateConfig({
              fit_rejection_sigma: fit_rejection_sigma ?? 0,
            })} />
          <BackgroundNumberField label="Rejection passes"
            value={extraction.config.fit_rejection_iterations}
            min={0} max={16} step={1} disabled={disabled}
            onChange={(fit_rejection_iterations) => updateConfig({
              fit_rejection_iterations: fit_rejection_iterations ?? 0,
            })} />
          <BackgroundNumberField label="Border fraction" value={extraction.config.border_fraction}
            min={0} max={0.449} step={0.01} disabled={disabled}
            onChange={(border_fraction) => updateConfig({ border_fraction: border_fraction ?? 0 })} />
          {extraction.config.model.kind !== 'radial_basis' && (
            <BackgroundNumberField label="Ridge" value={extraction.config.model.ridge}
              min={0} step="any" disabled={disabled}
              onChange={(ridge) => extraction.config.model.kind === 'automatic'
                ? updateAutomaticModel({ ridge: ridge ?? 0 })
                : updatePolynomialModel({ ridge: ridge ?? 0 })} />
          )}
        </div>
      )}
      {extraction && (extraction.config.model.kind === 'radial_basis' ||
        (extraction.config.model.kind === 'automatic' &&
          extraction.config.model.allow_radial_basis)) && (
        <p>
          Radial-basis fitting is slower and can follow real nebulosity. Check the result against
          the original stack.
        </p>
      )}
      {fits.length > 0 && (
        <div className="stack-background-diagnostics" aria-label="Background fit diagnostics">
          {fits.map(([role, fit]) => (
            <span key={role}>
              <strong>{roleLabels[role]}</strong>
              {fit.diagnostics.accepted_samples}/{fit.diagnostics.candidate_samples} samples
              <small>
                {fit.diagnostics.model_selection?.selected ?? fit.model.kind}
                {' · '}radius {fit.diagnostics.sample_radius}
                {' · '}{fit.diagnostics.protected_regions ?? 0} protected
              </small>
              {fallbacks[role] ? (
                <small title={`Protected fit failed: ${fallbacks[role]}`}>
                  Catalog protection skipped after the protected fit failed
                </small>
              ) : !!protections[role]?.object_names.length && (
                <small title={protections[role]?.object_names.join(', ')}>
                  Protected: {protections[role]?.object_names.join(', ')}
                </small>
              )}
            </span>
          ))}
        </div>
      )}
    </section>
  );
}

function StageLane({
  label, stages, channels, deconvolution, deconvolutionResult, rcAstro, rcAstroResult, inherited,
  disabled, onChange, onDeconvolutionChange, onRcAstroChange,
}: {
  label: string;
  stages: StackStretchRequest[];
  channels: 1 | 3;
  /** While the build follows the channel cards: what this input's card
   *  applied, in words, shown in place of the tool editors (`label`, null
   *  when the card applied no tools). `current` is present when the card
   *  now applies something else than the build ran. */
  inherited?: InheritedTools;
  deconvolution?: StackColorProcessing['input_deconvolutions'][StackColorRole];
  deconvolutionResult?: StackDeconvolutionResult;
  rcAstro?: NonNullable<StackColorProcessing['input_rc_astro']>[StackColorRole];
  rcAstroResult?: NonNullable<StackColorJob['resolved_input_rc_astro']>[StackColorRole];
  disabled: boolean;
  onChange: (stages: StackStretchRequest[]) => void;
  onDeconvolutionChange?: (
    config: StackColorProcessing['input_deconvolutions'][StackColorRole] | null
  ) => void;
  onRcAstroChange?: (
    config: NonNullable<StackColorProcessing['input_rc_astro']>[StackColorRole] | null
  ) => void;
}) {
  const replace = (index: number, stage: StackStretchRequest) => {
    const next = [...stages];
    next[index] = stage;
    onChange(next);
  };
  const move = (index: number, direction: -1 | 1) => {
    const destination = index + direction;
    if (destination < 0 || destination >= stages.length) return;
    const next = [...stages];
    [next[index], next[destination]] = [next[destination], next[index]];
    onChange(next);
  };

  return (
    <section className="stack-color-stage-lane" aria-label={`${label} stretch stack`}>
      <header>
        <strong>{label}</strong>
        <span>{stages.length} stage{stages.length === 1 ? '' : 's'}</span>
      </header>
      {inherited && (
        <p className="processing-setups-hint">
          {inherited.label
            ? `From its channel card: ${inherited.label}.`
            : 'Its channel card applied no deconvolution or RC-Astro steps.'}
          {inherited.current !== undefined && (
            <>
              {' '}
              {inherited.current
                ? `The card now applies: ${inherited.current}.`
                : 'The card now applies no deconvolution or RC-Astro steps.'}
            </>
          )}
        </p>
      )}
      {inherited && deconvolutionResult && (
        <StackDeconvolutionSummary label={label} result={deconvolutionResult} />
      )}
      {onDeconvolutionChange && (
        <StackDeconvolutionControls
          label={label}
          config={deconvolution}
          result={deconvolutionResult}
          disabled={disabled}
          onChange={onDeconvolutionChange}
        />
      )}
      {onRcAstroChange && (
        <StackRcAstroControls label={label} config={rcAstro} result={rcAstroResult}
          disabled={disabled} onChange={onRcAstroChange} />
      )}
      {rcAstroResult && (
        <div className="stack-stretch-actions">
          <a href={rcAstroResult.fits_url} download>
            {rcAstroResult.has_stars ? 'Starless FITS' : 'Processed FITS'}
          </a>
          {rcAstroResult.stars_fits_url && (
            <a href={rcAstroResult.stars_fits_url} download>Stars FITS</a>
          )}
        </div>
      )}
      {stages.length === 0 && (
        <p>Normalized only — no stretch stages.</p>
      )}
      {stages.map((stage, index) => (
        <div className="stack-color-stage" key={`${index}:${stage.model.type}`}>
          <div className="stack-color-stage-heading">
            <span>Stage {index + 1} · {stretchModelLabels[stage.model.type]}</span>
            <div>
              <button type="button" disabled={disabled || index === 0}
                aria-label={`Move ${label} stage ${index + 1} earlier`}
                onClick={() => move(index, -1)}>↑</button>
              <button type="button" disabled={disabled || index === stages.length - 1}
                aria-label={`Move ${label} stage ${index + 1} later`}
                onClick={() => move(index, 1)}>↓</button>
              <button type="button" disabled={disabled}
                aria-label={`Remove ${label} stage ${index + 1}`}
                onClick={() => onChange(stages.filter((_, candidate) => candidate !== index))}>
                Remove
              </button>
            </div>
          </div>
          <StackStretchStageEditor
            label={`${label} stage ${index + 1}`}
            channels={channels}
            request={stage}
            disabled={disabled}
            onChange={(next) => replace(index, next)}
          />
        </div>
      ))}
      <button className="stack-color-add-stage" type="button" disabled={disabled}
        onClick={() => onChange([...stages, defaultStretchRequest('auto-mtf')])}>
        Add stage
      </button>
    </section>
  );
}

interface InheritedTools {
  label: string | null;
  current?: string | null;
}

/** A channel's tools as the server compares them: steps without settings
 *  count as none. */
function sameChannelTools(
  left: ChannelPixelProcessing | undefined,
  right: ChannelPixelProcessing | undefined
): boolean {
  const comparable = (channel: ChannelPixelProcessing | undefined) => JSON.stringify({
    deconvolution: channel?.deconvolution ?? undefined,
    rc_astro: channel?.rc_astro?.steps.length ? channel.rc_astro : undefined,
  });
  return comparable(left) === comparable(right);
}

/** What each channel card shows that a color input can take, in words:
 *  `Hα: BlurXTerminator, NoiseXTerminator · OIII: deconvolution`. */
function channelProcessingSummary(
  channels: ChannelProcessing,
  roles: StackColorRole[],
  toolName: (tool: string) => string
): string {
  return roles.flatMap((role) => {
    const tools = channelToolsLabel(channels[role], toolName);
    return tools ? [`${roleLabels[role]}: ${tools}`] : [];
  }).join(' · ');
}

export default function StackColorProcessingControls({
  label, roles, applied, backgrounds, protections, fallbacks, deconvolutions, rcAstro = {},
  colorRcAstro, channelProcessing = {}, followed, disabled, applyError, onApply,
}: {
  label: string;
  roles: StackColorRole[];
  applied: StackColorProcessing | null;
  backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  protections: Partial<Record<StackColorRole, StackBackgroundProtection>>;
  fallbacks: Partial<Record<StackColorRole, string>>;
  deconvolutions: Partial<Record<StackColorRole, StackDeconvolutionResult>>;
  rcAstro?: StackColorJob['resolved_input_rc_astro'];
  /** What RC-Astro did on the color channels together. */
  colorRcAstro?: StackColorJob['resolved_color_rc_astro'];
  /** The deconvolution and RC-Astro steps each channel card shows, which
   *  "Use each channel's processing" copies into the inputs. */
  channelProcessing?: ChannelProcessing;
  /** For a build that followed the cards: what each card had applied when
   *  it was built, which is what its inputs ran. */
  followed?: ChannelProcessing;
  disabled: boolean;
  /** Why the server refused the last Apply, shown beside the button. */
  applyError?: string | null;
  onApply: (processing: StackColorProcessing) => void;
}) {
  const rcAstroCapabilities = useRcAstroCapabilities().data;
  const rcAstroTools = rcAstroCapabilities?.tools;
  const calibrates = colorCalibrationApplies(roles);
  const defaults = useMemo(() => defaultColorProcessing(roles), [roles]);
  const baseline = applied
    ? { ...applied, input_deconvolutions: applied.input_deconvolutions ?? {},
        input_rc_astro: applied.input_rc_astro ?? {} }
    : defaults;
  const [draft, setDraft] = useState<StackColorProcessing>(() => cloneProcessing(baseline));
  const [error, setError] = useState<string | null>(null);
  const changed = canonicalJson(draft) !== canonicalJson(baseline);
  const toolName = (tool: string) => rcAstroTools?.find((schema) => schema.key === tool)?.name ?? tool;
  const channelSummary = channelProcessingSummary(channelProcessing, roles, toolName);
  const followsCards = draft.input_processing === 'channels';
  // What each lane says its card applied: what the build ran, when it
  // followed the cards, and what the card applies now when that differs.
  // Before a build follows them, the cards' current steps are what it runs.
  const builtFromCards = applied?.input_processing === 'channels' ? followed : undefined;
  const inheritedFor = (role: StackColorRole): InheritedTools => {
    const live = channelProcessing[role];
    if (!builtFromCards) return { label: channelToolsLabel(live, toolName) };
    const ran = builtFromCards[role];
    const label = channelToolsLabel(ran, toolName);
    if (sameChannelTools(ran, live)) return { label };
    const now = channelToolsLabel(live, toolName);
    return { label, current: now !== null && now === label ? `${now}, with other settings` : now };
  };

  const updateInput = (role: StackColorRole, stages: StackStretchRequest[]) => setDraft((current) => ({
    ...current,
    input_stretches: { ...current.input_stretches, [role]: stages },
  }));
  const updateDeconvolution = (
    role: StackColorRole,
    config: StackColorProcessing['input_deconvolutions'][StackColorRole] | null
  ) => setDraft((current) => {
    const input_deconvolutions = { ...current.input_deconvolutions };
    if (config) input_deconvolutions[role] = config;
    else delete input_deconvolutions[role];
    return { ...current, input_deconvolutions };
  });
  const updateRcAstro = (
    role: StackColorRole,
    config: NonNullable<StackColorProcessing['input_rc_astro']>[StackColorRole] | null
  ) => setDraft((current) => {
    const input_rc_astro = { ...current.input_rc_astro };
    if (config) input_rc_astro[role] = config;
    else delete input_rc_astro[role];
    return { ...current, input_rc_astro };
  });
  // Off leaves the field out, as the server writes it, so a draft turned on
  // and off again matches what was applied.
  const updateColorRcAstro = (config: StackColorProcessing['color_rc_astro']) =>
    setDraft((current) => {
      if (config) return { ...current, color_rc_astro: config };
      const next = { ...current };
      delete next.color_rc_astro;
      return next;
    });
  const submit = () => {
    if (hasInvalidNumbers(draft)) {
      setError('Enter a finite value for every processing parameter');
      return;
    }
    const backgroundError = validateBackground(draft.background_extraction);
    if (backgroundError) {
      setError(backgroundError);
      return;
    }
    // Following the cards, the server takes each input's tools from its
    // card, so the build's own tool settings are not checked or used.
    if (!followsCards) {
      for (const config of Object.values(draft.input_deconvolutions)) {
        const deconvolutionError = validateDeconvolution(config);
        if (deconvolutionError) {
          setError(deconvolutionError);
          return;
        }
      }
      for (const role of roles) {
        const problem = rcAstroProblem(draft.input_rc_astro?.[role], rcAstroTools);
        if (problem) {
          setError(`${roleLabels[role]}: ${problem}`);
          return;
        }
      }
    }
    const colorProblem = rcAstroProblem(draft.color_rc_astro, rcAstroTools);
    if (colorProblem) {
      setError(`Color channels: ${colorProblem}`);
      return;
    }
    setError(null);
    onApply(cloneProcessing(draft));
  };

  return (
    <details className="stack-color-processing" aria-label={`${label} processing stack`}>
      <summary>
        <span>Processing stack</span>
        <small>BG {draft.background_extraction ? draft.background_extraction.correction_mode : 'off'}
          {followsCards
            ? ' · Tools from channel cards'
            : <>
              {' · '}DC {Object.keys(draft.input_deconvolutions).length || 'off'}
              {' · '}RC-Astro {Object.values(draft.input_rc_astro ?? {}).reduce(
                (count, config) => count + (config?.steps.length ?? 0), 0
              ) || 'off'}
            </>}
          {!!draft.color_rc_astro?.steps.length &&
            ` · Color RC-Astro ${draft.color_rc_astro.steps.length}`}
          {calibrates && ` · Calibration ${draft.color_calibration?.enabled === false ? 'off' : 'on'}`}
          {' · '}{roles.map((role) => `${roleLabels[role]} ${draft.input_stretches[role]?.length ?? 0}`).join(' · ')}
          {' · '}RGB {draft.output_stretches.length}</small>
      </summary>
      <div className="stack-color-processing-body">
        <ProcessingSetupsBar
          kind="color"
          builtins={builtinColorSetups.map((setup) => ({
            name: setup.name,
            settings: setup.settings(roles),
          }))}
          current={() => draft}
          disabled={disabled}
          onApply={(settings) => {
            setDraft(cloneProcessing(
              colorSetupForRoles(settings as StackColorProcessing, roles)
            ));
            setError(null);
          }}
        />
        <div className="processing-channel-bar" role="group" aria-label="Channel processing">
          <div className="processing-setups-row">
            <span className="processing-setups-label">Deconvolution and RC-Astro</span>
            <div className="tool-segmented" role="radiogroup" aria-label="Deconvolution and RC-Astro steps">
              <button
                type="button"
                role="radio"
                aria-checked={followsCards}
                disabled={disabled}
                onClick={() => {
                  setDraft((current) => ({ ...current, input_processing: 'channels' }));
                  setError(null);
                }}
              >
                From each channel card
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={!followsCards}
                disabled={disabled}
                onClick={() => {
                  if (followsCards) {
                    setDraft((current) => ownFromChannels(current, channelProcessing, roles));
                  }
                  setError(null);
                }}
              >
                Set in this build
              </button>
            </div>
          </div>
          <span className="processing-setups-hint">
            {!followsCards
              ? 'Each input runs the deconvolution and RC-Astro steps set in its lane below.'
              : channelSummary
                ? `Each input runs the steps its channel card applied (${channelSummary}). ` +
                  'Changing a card makes this preview out of date.'
                : 'No channel card has deconvolution or RC-Astro steps applied, so the inputs run none.'}
          </span>
        </div>
        <BackgroundControls
          extraction={draft.background_extraction}
          backgrounds={backgrounds}
          protections={protections}
          fallbacks={fallbacks}
          disabled={disabled}
          onChange={(background_extraction) => setDraft((current) => ({
            ...current, background_extraction,
          }))}
        />
        {(rcAstroCapabilities?.available || !!draft.color_rc_astro?.steps.length ||
          colorRcAstro) && (
          <section className="stack-color-channel-tools" aria-label="RC-Astro on the color channels together">
            <StackRcAstroControls
              label="Color channels"
              heading="RC-Astro on the color channels together"
              config={draft.color_rc_astro}
              result={colorRcAstro ?? undefined}
              disabled={disabled}
              toolKeys={colorChannelTools}
              defaultsFor={colorChannelDefaults}
              onChange={updateColorRcAstro}
            />
            <span className="processing-setups-hint">
              Runs on the aligned color channels together, before color calibration and
              the stretches. Correct only fixes star shapes and color fringes;
              NoiseXTerminator also reduces color noise.
            </span>
            {colorRcAstro && (
              <div className="stack-stretch-actions">
                <a href={colorRcAstro.fits_url} download>Processed color channels FITS</a>
              </div>
            )}
          </section>
        )}
        {calibrates && (
          <ColorCalibrationSwitch
            calibration={draft.color_calibration}
            disabled={disabled}
            onChange={(color_calibration) => setDraft((current) => ({
              ...current, color_calibration,
            }))}
          />
        )}
        <div className="stack-color-stage-grid">
          {roles.map((role) => (
            <StageLane
              key={role}
              label={`${roleLabels[role]} input`}
              stages={draft.input_stretches[role] ?? []}
              channels={1}
              deconvolution={draft.input_deconvolutions[role]}
              deconvolutionResult={deconvolutions[role]}
              rcAstro={draft.input_rc_astro?.[role]}
              rcAstroResult={rcAstro[role]}
              inherited={followsCards ? inheritedFor(role) : undefined}
              disabled={disabled}
              onChange={(stages) => updateInput(role, stages)}
              onDeconvolutionChange={followsCards ? undefined
                : (config) => updateDeconvolution(role, config)}
              onRcAstroChange={followsCards ? undefined : (config) => updateRcAstro(role, config)}
            />
          ))}
          <StageLane
            label="RGB output"
            stages={draft.output_stretches}
            channels={3}
            disabled={disabled}
            onChange={(output_stretches) => setDraft((current) => ({
              ...current, output_stretches,
            }))}
          />
        </div>
        {(error ?? applyError) && (
          <div className="stack-stretch-error" role="alert">{error ?? applyError}</div>
        )}
        <div className="stack-stretch-actions">
          <button type="button" disabled={disabled || !changed} onClick={submit}>
            Apply processing stack
          </button>
          <button type="button" disabled={disabled || !changed}
            onClick={() => { setDraft(cloneProcessing(baseline)); setError(null); }}>
            Revert edits
          </button>
          <button type="button" disabled={disabled}
            onClick={() => { setDraft(cloneProcessing(defaults)); setError(null); }}>
            Reset defaults
          </button>
        </div>
      </div>
    </details>
  );
}
