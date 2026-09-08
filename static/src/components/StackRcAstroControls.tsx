import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
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
  onChange,
}: {
  label: string;
  config: RcAstroProcessing | null | undefined;
  result?: StackRcAstroResult;
  disabled: boolean;
  onChange: (config: RcAstroProcessing | null) => void;
}) {
  const capabilities = useQuery({
    queryKey: ['rc-astro-capabilities'],
    queryFn: apiClient.getRcAstroCapabilities,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });

  if (!capabilities.data?.available) {
    if (capabilities.data?.error || capabilities.error || config?.steps.length || result) {
      return (
        <section className="stack-rc-astro-controls" aria-label={`${label} RC-Astro tools`}>
          <header>
            <strong>RC-Astro tools</strong>
            <span>{capabilities.isPending ? 'Checking tools' : 'Unavailable'}</span>
          </header>
          <p>{capabilities.data?.error ?? capabilities.error?.message ??
            (capabilities.isPending ? 'Checking server availability.' :
              'RC-Astro is not installed on this server. Saved steps are retained.')}</p>
          {!!config?.steps.length && (
            <>
              <ol>{orderedSteps(config.steps).map((step) => (
                <li key={step.tool}>{step.tool.toUpperCase()}
                  {Object.entries(step.parameters).map(([name, value]) => (
                    <small key={name}> {name}: {String(value)}</small>
                  ))}
                </li>
              ))}</ol>
              <button type="button" disabled={disabled} onClick={() => onChange(null)}>
                Remove RC-Astro steps
              </button>
            </>
          )}
          {result && <ResultDiagnostics result={result} />}
        </section>
      );
    }
    return null;
  }
  const tools = capabilities.data.tools;

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
        <strong>RC-Astro tools</strong>
        <span>
          {config?.steps.length
            ? `${config.steps.length} step(s) · linear, before stretch`
            : 'Off'}
        </span>
      </header>
      {!!config?.steps.length && (
        <p>{orderedSteps(config.steps).map((step) =>
          tools.find((schema) => schema.key === step.tool)?.name ?? step.tool
        ).join(' → ')}</p>
      )}
      {tools.map((schema) => (
        <ToolSection
          key={schema.key}
          schema={schema}
          parameters={stepFor(schema.key)?.parameters}
          disabled={disabled}
          onToggle={(enabled) =>
            setStep(schema.key, enabled ? defaultParameters(schema) : null)
          }
          onParameter={(name, value) => {
            const current = stepFor(schema.key)?.parameters ?? defaultParameters(schema);
            setStep(schema.key, { ...current, [name]: value });
          }}
        />
      ))}
      {config?.steps.filter((step) => !tools.some((tool) => tool.key === step.tool)).map((step) => (
        <label key={step.tool}>
          <input type="checkbox" checked disabled={disabled}
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

/** What a freshly enabled tool asks for: star removal keeps the stars. */
function defaultParameters(schema: ExternalToolSchema): Record<string, RcAstroParameterValue> {
  if (schema.key === 'sxt') {
    const stars = schema.parameters.find(
      (parameter) => parameter.name === 'stars' || parameter.name === 'difference'
    );
    if (stars?.flag) return { [stars.name]: true };
  }
  return {};
}

function ToolSection({
  schema,
  parameters,
  disabled,
  onToggle,
  onParameter,
}: {
  schema: ExternalToolSchema;
  parameters: Record<string, RcAstroParameterValue> | undefined;
  disabled: boolean;
  onToggle: (enabled: boolean) => void;
  onParameter: (name: string, value: RcAstroParameterValue) => void;
}) {
  const enabled = parameters !== undefined;
  return (
    <div className="stack-rc-astro-tool">
      <label>
        <input
          type="checkbox"
          checked={enabled}
          disabled={disabled || (!schema.licensed && !enabled)}
          onChange={(event) => onToggle(event.target.checked)}
          aria-label={schema.name}
        />
        <strong>{schema.name}</strong>
        {!schema.licensed && <small>not licensed</small>}
      </label>
      {enabled && (
        <div className="stack-rc-astro-fields">
          {schema.parameters
            .filter((parameter) => parameter.flag !== null)
            .map((parameter) => (
              <ParameterField
                key={parameter.name}
                parameter={parameter}
                value={parameters[parameter.name]}
                disabled={disabled || !schema.licensed}
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
  return (
    <label className="stack-stretch-field" title={parameter.description}>
      <span>{parameter.label}</span>
      <input
        type="number"
        aria-label={parameter.label}
        value={Number.isFinite(numeric) ? numeric : ''}
        min={kind.min}
        max={kind.max}
        step={kind.type === 'int' ? 1 : 0.05}
        disabled={disabled}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
    </label>
  );
}
