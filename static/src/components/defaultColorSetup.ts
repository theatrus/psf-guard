import { useCallback } from 'react';
import { useQuery, type QueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  ProcessingSetupsDocument,
  StackColorProcessing,
  StackColorRole,
} from '../api/types';
import { colorSetupForRoles } from './processingSetups';
import { ALL_COLOR_ROLES, defaultColorProcessing } from './stackColorProcessing';

export const PROCESSING_SETUPS_QUERY_KEY = ['processing-setups'] as const;

/** The setups list, shared by the Setups bar, Settings, and every color build. */
export function processingSetupsQuery() {
  return {
    queryKey: PROCESSING_SETUPS_QUERY_KEY,
    queryFn: apiClient.getProcessingSetups,
    staleTime: 30_000,
  };
}

/**
 * The pipeline a new color build of these channels starts from: the server's
 * default Color setup, or the built-in pipeline while the list is not loaded
 * or comes from a server that does not mark one.
 */
export function defaultColorFor(
  document: ProcessingSetupsDocument | undefined,
  roles: StackColorRole[]
): StackColorProcessing {
  const marked = document?.default_color_processing;
  return marked ? colorSetupForRoles(marked, roles) : defaultColorProcessing(roles);
}

/** {@link defaultColorFor}, following the server's list as it changes. */
export function useDefaultColorProcessing(): (roles: StackColorRole[]) => StackColorProcessing {
  const { data } = useQuery(processingSetupsQuery());
  return useCallback((roles: StackColorRole[]) => defaultColorFor(data, roles), [data]);
}

/**
 * The pipeline a build sends for a target's first color preview, whichever
 * channels it turns out to have. It waits for the server's list, so a build
 * started at once still uses the marked setup; when the list cannot be read,
 * it uses the built-in pipeline.
 */
export async function fetchColorDefaults(queryClient: QueryClient): Promise<StackColorProcessing> {
  try {
    const document = await queryClient.fetchQuery(processingSetupsQuery());
    return document.default_color_processing ?? defaultColorProcessing(ALL_COLOR_ROLES);
  } catch {
    return defaultColorProcessing(ALL_COLOR_ROLES);
  }
}
