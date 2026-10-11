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
  if (kind.type === 'int' && !Number.isInteger(value)) return `${parameter.label} takes a whole number`;
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
