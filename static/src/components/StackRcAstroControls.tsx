import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  ExternalToolParameter,
  ExternalToolSchema,
  RcAstroProcessing,
  RcAstroParameterValue,
  StackRcAstroResult,
} from '../api/types';

/**
 * RC-Astro tool chain controls (BlurXTerminator, NoiseXTerminator,
 * StarXTerminator), rendered from each tool's live schema so the knobs stay
 * correct across CLI upgrades. Nothing renders when the CLI is not
 * installed on the server.
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

  // No install renders nothing; an install whose probe failed says so, so
  // a broken setup is distinguishable from an absent one.
  if (!capabilities.data?.available) {
    if (capabilities.data?.error) {
      return (
        <section className="stack-rc-astro-controls" aria-label={`${label} RC-Astro tools`}>
          <header>
            <strong>RC-Astro tools</strong>
            <span>unavailable</span>
          </header>
          <p>{capabilities.data.error}</p>
        </section>
      );
    }
    return null;
  }
  const tools = capabilities.data.tools;

  const stepFor = (tool: string) => config?.steps.find((step) => step.tool === tool);

  const setStep = (tool: string, parameters: Record<string, RcAstroParameterValue> | null) => {
    const others = config?.steps.filter((step) => step.tool !== tool) ?? [];
    const steps = parameters === null ? others : [...others, { tool, parameters }];
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
      <p>
        Runs the server&apos;s licensed RC-Astro tools on the linear stack.
        Star removal keeps both images, so starless and stars stretch on
        their own.
      </p>
      {tools.map((schema) => (
        <ToolSection
          key={schema.key}
          schema={schema}
          parameters={stepFor(schema.key)?.parameters}
          disabled={disabled || !schema.licensed}
          onToggle={(enabled) =>
            setStep(schema.key, enabled ? defaultParameters(schema) : null)
          }
          onParameter={(name, value) => {
            const current = stepFor(schema.key)?.parameters ?? defaultParameters(schema);
            setStep(schema.key, { ...current, [name]: value });
          }}
        />
      ))}
      {result && (
        <div className="stack-rc-astro-diagnostics">
          {result.steps.map((step) => (
            <span key={step.tool}>
              {step.name}
              {step.device ? ` · ${step.device}` : ''}
              {step.warnings.map((warning) => (
                <small key={warning}>{warning}</small>
              ))}
            </span>
          ))}
        </div>
      )}
    </section>
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
          disabled={disabled}
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
                disabled={disabled}
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
