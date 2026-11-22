import { Fragment } from 'react';
import {
  defaultParameters,
  isCorrectOnly,
  offersCorrectOnly,
  useRcAstroCapabilities,
  bxtCorrectOnly,
} from './rcAstroSchema';
import ParameterField from './RcAstroParameterField';
import {
  categoryOf,
  orderedSteps,
  PROCESSING_CATEGORIES,
} from './processingTools';
import { toolName } from './stackProcessingLabel';
import { openSettings } from '../utils/settingsIntent';
import type {
  ProcessingCategory,
  ExternalToolSchema,
  ProcessingProvider,
  RcAstroProcessing,
  RcAstroParameterValue,
  RcAstroStep,
  StackRcAstroResult,
} from '../api/types';

interface Props {
  label: string;
  config: RcAstroProcessing | null | undefined;
  result?: StackRcAstroResult;
  disabled: boolean;
  /** The categories offered; every provider the server lists when absent. */
  categories?: readonly ProcessingCategory[];
  /** The parameters a provider starts with when it is picked. */
  defaultsFor?: (schema: ExternalToolSchema) => Record<string, RcAstroParameterValue>;
  heading?: string;
  /** Required managed steps are configured here; the star policy controls running them. */
  required?: boolean;
  excludedParameters?: readonly string[];
  onChange: (config: RcAstroProcessing | null) => void;
}

const stateMark: Record<ProcessingProvider['state'], string> = {
  ready: '',
  not_licensed: ' (not licensed)',
  not_installed: ' (not installed)',
  excluded: ' (not used)',
};

/**
 * The processing steps that run on linear data before the stretch. Each
 * category (sharpening, noise reduction, star removal) has one control. With
 * one provider to pick it is a checkbox with the provider's name; with more
 * it is a picker: Off, or one of the providers the server lists. A chain
 * never has two steps of one category. The picked provider's controls come from its live
 * schema, so they stay correct across upgrades. A saved step stays visible
 * and removable when its provider is gone.
 */
export default function StackRcAstroControls({
  label,
  config,
  result,
  disabled,
  categories,
  defaultsFor = defaultParameters,
  heading = 'Processing tools',
  required = false,
  excludedParameters = [],
  onChange,
}: Props) {
  const capabilities = useRcAstroCapabilities();
  const listed = capabilities.data?.providers ?? [];
  const offered = listed.filter((provider) => !categories || categories.includes(provider.category));
  const steps = config?.steps ?? [];
  const problem = capabilities.data?.error ?? capabilities.error?.message;

  if (!offered.length && !steps.length && !result && !problem) return null;

  const categoryOfStep = (step: RcAstroStep) => categoryOf(step.tool, listed);
  const nameOfStep = (step: RcAstroStep) => {
    const provider = listed.find((candidate) => candidate.provider === step.tool);
    return provider ? toolName(provider.name) : step.tool;
  };
  const removeStep = (step: RcAstroStep) => {
    const next = steps.filter((candidate) => candidate !== step);
    onChange(next.length ? { steps: next } : null);
  };
  // Steps PSF Guard cannot place in a category stay listed on their own.
  const uncategorized = steps.filter((step) => !categoryOfStep(step));
  const blocks = PROCESSING_CATEGORIES.flatMap(({ category, label: categoryLabel }) => {
    const providers = offered.filter((provider) => provider.category === category);
    const saved = steps.filter((step) => categoryOfStep(step) === category);
    return providers.length || saved.length
      ? [{ category, categoryLabel, providers, step: saved[0], extra: saved.slice(1) }]
      : [];
  });

  /** Replace a category's step, or with `null` remove it. */
  const setCategoryStep = (category: string, step: RcAstroStep | null) => {
    const others = steps.filter((candidate) => categoryOfStep(candidate) !== category);
    const next = step ? orderedSteps([...others, step], listed) : others;
    onChange(next.length ? { steps: next } : null);
  };
  /** Edit a step's parameters where it stands. */
  const setParameters = (tool: string, parameters: Record<string, RcAstroParameterValue>) => {
    onChange({ steps: steps.map((step) => step.tool === tool ? { ...step, parameters } : step) });
  };

  const ready = offered.filter((provider) => provider.state === 'ready');
  const onWorkers = ready.some((provider) => provider.workers.length);

  return (
    <section className="stack-rc-astro-controls" aria-label={`${label} processing tools`}>
      <header>
        <strong>{heading}</strong>
        <span>
          {capabilities.isPending
            ? 'Checking tools'
            : steps.length
              ? `${steps.length} step(s) · linear, before stretch`
              : 'Off'}
        </span>
      </header>
      {problem && <p>{problem}</p>}
      {onWorkers && (
        <p className="stack-rc-astro-where">
          {ready.map((provider) =>
            `${toolName(provider.name)} on ${provider.workers.length ? provider.workers.join(', ') : 'this server'}`
          ).join(' · ')}
          {ready.some((provider) => provider.workers.length && provider.this_server)
            && '. This server runs a tool while no worker that takes it is connected.'}
        </p>
      )}
      {blocks.map(({ category, categoryLabel, providers, step, extra }) => {
        const picked = step ? providers.find((provider) => provider.provider === step.tool) : undefined;
        // What a person could pick: the ready providers, and the one the
        // step uses now, so a saved choice is never hidden.
        const choices = providers.filter(
          (provider) => provider.state === 'ready' || provider === picked
        );
        const choiceCount = choices.length + (step && !picked ? 1 : 0);
        // The single choice; absent when it is a step the server does not list.
        const only = step ? picked : choices[0];
        const title = required ? 'The Stars choice controls whether star separation runs' : undefined;
        const stepOf = (provider: ProcessingProvider): RcAstroStep => ({
          tool: provider.provider,
          parameters: provider.schema ? defaultsFor(provider.schema) : {},
        });
        // Providers a person cannot pick, other than the one in use.
        const others = providers.filter(
          (provider) => provider.state !== 'ready' && provider !== picked
        );
        return (
          <div className="stack-rc-astro-tool" key={category} role="group" aria-label={categoryLabel}>
            {choiceCount > 1 && (
              <label title={title}>
                <strong>{categoryLabel}</strong>
                <select
                  aria-label={categoryLabel}
                  value={step?.tool ?? ''}
                  disabled={disabled || required}
                  onChange={(event) => {
                    const provider = providers.find((candidate) => candidate.provider === event.target.value);
                    setCategoryStep(category, provider ? stepOf(provider) : null);
                  }}
                >
                  <option value="">Off</option>
                  {providers.map((provider) => (
                    <option key={provider.provider} value={provider.provider}
                      disabled={provider.state !== 'ready' && provider.provider !== step?.tool}>
                      {toolName(provider.name)}{stateMark[provider.state]}
                    </option>
                  ))}
                  {step && !picked && (
                    <option value={step.tool}>
                      {capabilities.isPending ? step.tool : `${step.tool} (unavailable)`}
                    </option>
                  )}
                </select>
              </label>
            )}
            {/* One choice: a checkbox with the provider's name, as the cards
                had before a category could have several providers. */}
            {choiceCount === 1 && (
              <label title={title}>
                <input
                  type="checkbox"
                  checked={!!step}
                  disabled={disabled || required}
                  onChange={(event) =>
                    setCategoryStep(category, event.target.checked && only ? stepOf(only) : null)}
                />
                <strong>
                  {only?.name ?? (capabilities.isPending ? step?.tool : `${step?.tool} (unavailable)`)}
                </strong>
              </label>
            )}
            {/* Nothing to pick: an unlicensed tool keeps its checkbox,
                disabled; a provider that is not installed has its note. */}
            {choiceCount === 0 && !others.some((provider) => provider.state === 'not_licensed') && (
              <strong>{categoryLabel}</strong>
            )}
            {step && !picked && (
              <p className="stack-rc-astro-note">
                {capabilities.isPending
                  ? 'Checking server availability.'
                  : 'Not available on this server or a connected worker. Your saved step is unchanged.'}
                {' '}{parameterList(step)}
              </p>
            )}
            {picked && picked.state !== 'ready' && <ProviderNote provider={picked} />}
            {/* A chain saved before one step per category was the rule. */}
            {extra.map((other) => (
              <p className="stack-rc-astro-note" key={other.tool}>
                This chain also runs {nameOfStep(other)}.{' '}
                {!required && (
                  <button type="button" disabled={disabled}
                    aria-label={`Remove ${nameOfStep(other)}`}
                    onClick={() => removeStep(other)}>
                    Remove
                  </button>
                )}
              </p>
            ))}
            {step && picked && (picked.schema ? (
              <ProviderFields
                schema={picked.schema}
                parameters={step.parameters}
                disabled={disabled || picked.state !== 'ready'}
                excludedParameters={excludedParameters}
                onParameters={(parameters) => setParameters(step.tool, parameters)}
              />
            ) : (
              <p className="stack-rc-astro-note">{parameterList(step)}</p>
            ))}
            {/* The picker lists these while a step is picked; a checkbox
                does not, so their notes stay under it. */}
            {(!step || choiceCount === 1) && others.map((provider) => (
              choiceCount === 0 && provider.state === 'not_licensed' ? (
                <Fragment key={provider.provider}>
                  <label>
                    <input type="checkbox" checked={false} disabled readOnly />
                    <strong>{provider.name}</strong>
                  </label>
                  <ProviderNote provider={provider} />
                </Fragment>
              ) : (
                <ProviderNote key={provider.provider} provider={provider} named />
              )
            ))}
          </div>
        );
      })}
      {uncategorized.map((step) => (
        <p className="stack-rc-astro-note" key={step.tool}>
          {step.tool} (unavailable) {parameterList(step)}{' '}
          {!required && (
            <button type="button" disabled={disabled}
              aria-label={`Remove ${step.tool}`}
              onClick={() => removeStep(step)}>
              Remove
            </button>
          )}
        </p>
      ))}
      {result && <ResultDiagnostics result={result} />}
    </section>
  );
}

function parameterList(step: RcAstroStep): string {
  return Object.entries(step.parameters)
    .map(([name, value]) => `${name}: ${String(value)}`)
    .join(' · ');
}

/** Why a provider cannot run, with the action when PSF Guard has one. */
function ProviderNote({ provider, named = false }: { provider: ProcessingProvider; named?: boolean }) {
  const reason = provider.reason
    ?? (provider.state === 'not_licensed' ? 'Not licensed.' : 'Not installed.');
  return (
    <p className="stack-rc-astro-note">
      {named && `${toolName(provider.name)}: `}{reason}{' '}
      {provider.remedy === 'download_noise_model' && (
        <button type="button" onClick={() => openSettings('processing-tools')}>
          Download in Settings
        </button>
      )}
      {provider.remedy === 'change_placement' && (
        <button type="button" onClick={() => openSettings('workers')}>
          Open Workers
        </button>
      )}
    </p>
  );
}

function ResultDiagnostics({ result }: { result: StackRcAstroResult }) {
  return (
    <div className="stack-rc-astro-diagnostics">
      {/* What made the result shown, which may no longer be installed. */}
      {result.cli_version && <span>Made with RC-Astro {result.cli_version}</span>}
      {result.steps.map((step) => (
        <span key={step.tool}>
          {step.name}
          {step.ml_version !== null && step.ml_version !== undefined ? ` · model ${step.ml_version}` : ''}
          {step.version ? ` · ${step.version}` : ''}
          {step.device ? ` · ${step.device}` : ''}
          {step.warnings.map((warning) => <small key={warning}>{warning}</small>)}
        </span>
      ))}
    </div>
  );
}

/** The picked provider's controls, drawn from its schema. */
function ProviderFields({
  schema,
  parameters,
  disabled,
  excludedParameters,
  onParameters,
}: {
  schema: ExternalToolSchema;
  excludedParameters: readonly string[];
  parameters: Record<string, RcAstroParameterValue>;
  disabled: boolean;
  onParameters: (parameters: Record<string, RcAstroParameterValue>) => void;
}) {
  const correctOnly = offersCorrectOnly(schema) && isCorrectOnly(parameters);
  return (
    <div className="stack-rc-astro-fields">
      {offersCorrectOnly(schema) && (
        <label className="stack-stretch-field stack-rc-astro-switch"
          title="Correct star shapes and color fringes without sharpening">
          <input
            type="checkbox"
            checked={correctOnly}
            disabled={disabled}
            onChange={(event) => onParameters(event.target.checked
              ? { ...parameters, ...bxtCorrectOnly }
              : Object.fromEntries(Object.entries(parameters)
                .filter(([name]) => !(name in bxtCorrectOnly))))}
            aria-label="Correct only"
          />
          <span>Correct only</span>
        </label>
      )}
      {schema.parameters
        .filter((parameter) => parameter.flag !== null && !excludedParameters.includes(parameter.name))
        .map((parameter) => (
          <ParameterField
            key={parameter.name}
            parameter={parameter}
            value={parameters[parameter.name]}
            disabled={disabled || (correctOnly && parameter.name in bxtCorrectOnly)}
            onChange={(value) => onParameters({ ...parameters, [parameter.name]: value })}
          />
        ))}
    </div>
  );
}
