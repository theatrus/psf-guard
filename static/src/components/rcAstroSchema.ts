import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  ExternalToolParameter,
  ExternalToolSchema,
  RcAstroParameterValue,
  RcAstroProcessing,
} from '../api/types';

/** The server's RC-Astro tools and their live schemas, shared across panels. */
export function useRcAstroCapabilities() {
  return useQuery({
    queryKey: ['rc-astro-capabilities'],
    queryFn: apiClient.getRcAstroCapabilities,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
}

/** Why a number does not fit its parameter, or null when it does. */
export function rangeProblem(parameter: ExternalToolParameter, value: RcAstroParameterValue): string | null {
  const { kind } = parameter;
  if (kind.type === 'bool' || typeof value !== 'number') return null;
  if (!Number.isFinite(value)) return `Enter a value for ${parameter.label}`;
  if (kind.type === 'int' && !Number.isInteger(value)) return `${parameter.label} requires a whole number`;
  if (value < kind.min || value > kind.max) {
    return `${parameter.label} must be between ${kind.min} and ${kind.max}`;
  }
  return null;
}

/**
 * The first parameter the tools would refuse, checked against their live
 * schemas, so a run is not started only for the server to turn it down.
 */
export function rcAstroProblem(
  config: RcAstroProcessing | null | undefined,
  tools: ExternalToolSchema[] | undefined,
): string | null {
  for (const step of config?.steps ?? []) {
    const schema = tools?.find((tool) => tool.key === step.tool);
    for (const [name, value] of Object.entries(step.parameters)) {
      if (typeof value === 'number' && !Number.isFinite(value)) {
        return 'Enter a finite value for every RC-Astro parameter';
      }
      const parameter = schema?.parameters.find((candidate) => candidate.name === name);
      const problem = parameter ? rangeProblem(parameter, value) : null;
      if (problem) return `${schema!.name}: ${problem}`;
    }
  }
  return null;
}

/**
 * BlurXTerminator's Correct only mode as parameter values. rc-astro's schema
 * lists the `--correct-only` switch without a type, so PSF Guard cannot send
 * it. The switch sets these values, and sending them gives the same pixels.
 */
export const bxtCorrectOnly: Record<string, RcAstroParameterValue> = {
  ss: 0, sn: 0, ash: 0, nsd: 0, ansp: true, lp: false,
};

/** Whether a schema can express Correct only: it has every pinned
 *  parameter, each settable from the CLI. */
export function offersCorrectOnly(schema: ExternalToolSchema): boolean {
  return schema.key === 'bxt' && Object.keys(bxtCorrectOnly).every((name) =>
    schema.parameters.some((parameter) => parameter.name === name && parameter.flag !== null)
  );
}

export function isCorrectOnly(parameters: Record<string, RcAstroParameterValue>): boolean {
  return Object.entries(bxtCorrectOnly).every(([name, value]) => parameters[name] === value);
}

/** What a freshly enabled tool asks for: star removal keeps the stars. */
export function defaultParameters(
  schema: ExternalToolSchema
): Record<string, RcAstroParameterValue> {
  if (schema.key === 'sxt') {
    const stars = schema.parameters.find(
      (parameter) => parameter.name === 'stars' || parameter.name === 'difference'
    );
    if (stars?.flag) return { [stars.name]: true };
  }
  return {};
}
