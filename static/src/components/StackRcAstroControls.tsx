import {
  defaultParameters,
  isCorrectOnly,
  offersCorrectOnly,
  rangeProblem,
  useRcAstroCapabilities,
  bxtCorrectOnly,
} from './rcAstroSchema';
import type {
  ExternalToolParameter,
  ExternalToolSchema,
  RcAstroProcessing,
  RcAstroParameterValue,
  StackRcAstroResult,
} from '../api/types';

const executionOrder = new Map([['bxt', 0], ['nxt', 1], ['sxt', 2]]);

function orderedSteps(steps: RcAstroProcessing['steps']): RcAstroProcessing['steps'] {
  return [...steps].sort((left, right) =>
    (executionOrder.get(left.tool) ?? 3) - (executionOrder.get(right.tool) ?? 3)
  );
}

/**
 * RC-Astro tool chain controls (BlurXTerminator, NoiseXTerminator,
 * StarXTerminator), rendered from each tool's live schema so the knobs stay
 * correct across CLI upgrades. Saved choices remain visible without a CLI.
 */
export default function StackRcAstroControls({
  label,
  config,
  result,
  disabled,
  toolKeys,
  defaultsFor = defaultParameters,
  heading = 'RC-Astro tools',
  required = false,
  excludedParameters = [],
  onChange,
}: {
  label: string;
  config: RcAstroProcessing | null | undefined;
  result?: StackRcAstroResult;
  disabled: boolean;
  /** The tools offered, by key; every tool the server reports when absent. */
  toolKeys?: readonly string[];
  /** The parameters a tool starts with when it is turned on. */
  defaultsFor?: (schema: ExternalToolSchema) => Record<string, RcAstroParameterValue>;
  heading?: string;
  /** Required managed steps are configured here; the star policy controls running them. */
  required?: boolean;
  excludedParameters?: readonly string[];
  onChange: (config: RcAstroProcessing | null) => void;
}) {
  const capabilities = useRcAstroCapabilities();

  if (!capabilities.data?.available) {
    if (capabilities.data?.error || capabilities.error || config?.steps.length || result) {
      return (
        <section className="stack-rc-astro-controls" aria-label={`${label} RC-Astro tools`}>
          <header>
            <strong>{heading}</strong>
            <span>{capabilities.isPending ? 'Checking tools' : 'Unavailable'}</span>
          </header>
          <p>{capabilities.data?.error ?? capabilities.error?.message ??
            (capabilities.isPending ? 'Checking server availability.' :
              'RC-Astro is not installed on this server or on a connected worker. Your saved processing steps are unchanged.')}</p>
          {!!config?.steps.length && (
            <>
              <ol>{orderedSteps(config.steps).map((step) => (
                <li key={step.tool}>{step.tool.toUpperCase()}
                  {Object.entries(step.parameters).map(([name, value]) => (
                    <small key={name}> {name}: {String(value)}</small>
                  ))}
                </li>
              ))}</ol>
              {!required && <button type="button" disabled={disabled} onClick={() => onChange(null)}>
                Remove RC-Astro steps
              </button>}
            </>
          )}
          {result && <ResultDiagnostics result={result} />}
        </section>
      );
    }
    return null;
  }
  const tools = capabilities.data.tools.filter(
    (schema) => !toolKeys || toolKeys.includes(schema.key)
  );

  const stepFor = (tool: string) => config?.steps.find((step) => step.tool === tool);

  const setStep = (tool: string, parameters: Record<string, RcAstroParameterValue> | null) => {
    const current = config?.steps ?? [];
    const steps = parameters === null
      ? current.filter((step) => step.tool !== tool)
      : current.some((step) => step.tool === tool)
        ? current.map((step) => step.tool === tool ? { tool, parameters } : step)
        : orderedSteps([...current, { tool, parameters }]);
    onChange(steps.length === 0 ? null : { steps });
  };

  return (
    <section className="stack-rc-astro-controls" aria-label={`${label} RC-Astro tools`}>
      <header>
        <strong>{heading}</strong>
        <span>
          {config?.steps.length
            ? `${config.steps.length} step(s) · linear, before stretch`
            : 'Off'}
        </span>
      </header>
      {!!capabilities.data.workers?.length && (
        <p className="stack-rc-astro-where">
          {tools.map((schema) => {
            const workers = capabilities.data.tool_workers?.[schema.key] ?? [];
            return `${schema.name.replace(/^RC-Astro /, '')} on ${workers.length ? workers.join(', ') : 'this server'}`;
          }).join(' · ')}
          {capabilities.data.this_server && tools.some((schema) => capabilities.data.tool_workers?.[schema.key]?.length)
            && '. This server runs a tool while no worker that takes it is connected.'}
        </p>
      )}
      {!!config?.steps.length && (
        <p>{orderedSteps(config.steps).map((step) =>
          tools.find((schema) => schema.key === step.tool)?.name ?? step.tool
        ).join(' → ')}</p>
      )}
      {tools.map((schema) => (
        <ToolSection
          key={schema.key}
          schema={schema}
          required={required}
          excludedParameters={excludedParameters}
          parameters={stepFor(schema.key)?.parameters}
          disabled={disabled}
          onToggle={(enabled) =>
            setStep(schema.key, enabled ? defaultsFor(schema) : null)
          }
          onParameter={(name, value) => {
            const current = stepFor(schema.key)?.parameters ?? defaultsFor(schema);
            setStep(schema.key, { ...current, [name]: value });
          }}
          onParameters={(parameters) => setStep(schema.key, parameters)}
        />
      ))}
      {config?.steps.filter((step) => !tools.some((tool) => tool.key === step.tool)).map((step) => (
        <label key={step.tool}>
          <input type="checkbox" checked disabled={disabled || required}
            aria-label={`${step.tool} unavailable`}
            onChange={() => setStep(step.tool, null)} />
          {step.tool} (unavailable)
        </label>
      ))}
      {result && <ResultDiagnostics result={result} />}
    </section>
  );
}

function ResultDiagnostics({ result }: { result: StackRcAstroResult }) {
  return (
    <div className="stack-rc-astro-diagnostics">
      <span>RC-Astro {result.cli_version}</span>
      {result.steps.map((step) => (
        <span key={step.tool}>
          {step.name}
          {step.ml_version !== null ? ` · model ${step.ml_version}` : ''}
          {step.device ? ` · ${step.device}` : ''}
          {step.warnings.map((warning) => <small key={warning}>{warning}</small>)}
        </span>
      ))}
    </div>
  );
}


function ToolSection({
  schema,
  parameters,
  disabled,
  required,
  excludedParameters,
  onToggle,
  onParameter,
  onParameters,
}: {
  schema: ExternalToolSchema;
  required: boolean;
  excludedParameters: readonly string[];
  parameters: Record<string, RcAstroParameterValue> | undefined;
  disabled: boolean;
  onToggle: (enabled: boolean) => void;
  onParameter: (name: string, value: RcAstroParameterValue) => void;
  onParameters: (parameters: Record<string, RcAstroParameterValue>) => void;
}) {
  const enabled = parameters !== undefined;
  const correctOnly = enabled && offersCorrectOnly(schema) && isCorrectOnly(parameters);
  return (
    <div className="stack-rc-astro-tool">
      <label title={required ? 'The Stars choice controls whether star separation runs' : undefined}>
        <input
          type="checkbox"
          checked={enabled}
          disabled={disabled || required || (!schema.licensed && !enabled)}
          onChange={(event) => onToggle(event.target.checked)}
          aria-label={schema.name}
        />
        <strong>{schema.name}</strong>
        {!schema.licensed && <small>not licensed</small>}
      </label>
      {enabled && (
        <div className="stack-rc-astro-fields">
          {offersCorrectOnly(schema) && (
            <label className="stack-stretch-field stack-rc-astro-switch"
              title="Correct star shapes and color fringes without sharpening">
              <input
                type="checkbox"
                checked={correctOnly}
                disabled={disabled || !schema.licensed}
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
                disabled={disabled || !schema.licensed ||
                  (correctOnly && parameter.name in bxtCorrectOnly)}
                onChange={(value) => onParameter(parameter.name, value)}
              />
            ))}
        </div>
      )}
    </div>
  );
}

function ParameterField({
  parameter,
  value,
  disabled,
  onChange,
}: {
  parameter: ExternalToolParameter;
  value: RcAstroParameterValue | undefined;
  disabled: boolean;
  onChange: (value: RcAstroParameterValue) => void;
}) {
  const { kind } = parameter;
  if (kind.type === 'bool') {
    return (
      <label className="stack-stretch-field stack-rc-astro-switch" title={parameter.description}>
        <input
          type="checkbox"
          checked={typeof value === 'boolean' ? value : kind.default}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
          aria-label={parameter.label}
        />
        <span>{parameter.label}</span>
      </label>
    );
  }
  const numeric = typeof value === 'number' ? value : kind.default;
  const problem = value === undefined ? null : rangeProblem(parameter, value);
  return (
    <label className="stack-stretch-field" title={parameter.description}>
      <span>{parameter.label}</span>
      <input
        type="number"
        aria-label={parameter.label}
        aria-invalid={problem ? true : undefined}
        value={Number.isFinite(numeric) ? numeric : ''}
        min={kind.min}
        max={kind.max}
        step={kind.type === 'int' ? 1 : 0.05}
        disabled={disabled}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
      {problem && (
        <small className="stack-rc-astro-field-error">{kind.min} to {kind.max}</small>
      )}
    </label>
  );
}
