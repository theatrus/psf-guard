import type {
  ExternalToolSchema,
  ProcessingCapabilities,
  ProcessingCategory,
  ProcessingProvider,
  RcAstroCapabilities,
  RcAstroProcessing,
} from '../api/types';

/** The query every view shares for the server's processing providers. */
export const processingCapabilitiesKey = ['rc-astro-capabilities'] as const;

/** The Settings field a processing card opens: the Processing tools section
 *  on the Stacking page. */
export const PROCESSING_TOOLS_FIELD = 'processing_tools';

/** Seiza's noise model among the providers. */
export const NOISE_MODEL_PROVIDER = 'noise-model';

/** The categories in the order a chain runs them. */
export const PROCESSING_CATEGORIES: ReadonlyArray<{ category: ProcessingCategory; label: string }> = [
  { category: 'sharpen', label: 'Sharpening' },
  { category: 'denoise', label: 'Noise reduction' },
  { category: 'star_removal', label: 'Star removal' },
];

/** The category of each provider PSF Guard knows, for a server that sends no
 *  provider list and for a saved step the server no longer lists. */
const knownCategories: Record<string, ProcessingCategory> = {
  bxt: 'sharpen',
  nxt: 'denoise',
  [NOISE_MODEL_PROVIDER]: 'denoise',
  sxt: 'star_removal',
};

/** A step's category: the server's when it lists the provider. */
export function categoryOf(
  tool: string,
  providers: readonly ProcessingProvider[] = [],
): ProcessingCategory | undefined {
  return providers.find((provider) => provider.provider === tool)?.category ?? knownCategories[tool];
}

function categoryRank(category: ProcessingCategory | undefined): number {
  const rank = PROCESSING_CATEGORIES.findIndex((entry) => entry.category === category);
  return rank < 0 ? PROCESSING_CATEGORIES.length : rank;
}

/** A chain's steps in run order: sharpen, denoise, then remove stars. */
export function orderedSteps(
  steps: RcAstroProcessing['steps'],
  providers: readonly ProcessingProvider[] = [],
): RcAstroProcessing['steps'] {
  return [...steps].sort((left, right) =>
    categoryRank(categoryOf(left.tool, providers)) - categoryRank(categoryOf(right.tool, providers))
  );
}

/**
 * The server's answer with `providers` always filled. A server older than
 * the provider list sends `tools` alone: each becomes a provider, ready when
 * it is licensed. A tool with no known category is left out.
 */
export function normalizeCapabilities(raw: RcAstroCapabilities): ProcessingCapabilities {
  const tools = raw.tools ?? [];
  const providers: ProcessingProvider[] = raw.providers
    ? raw.providers.map((provider) => ({
      ...provider,
      workers: provider.workers ?? [],
      schema: provider.schema ?? tools.find((schema) => schema.key === provider.provider),
    }))
    : tools.flatMap((schema): ProcessingProvider[] => {
      const category = knownCategories[schema.key];
      if (!category) return [];
      const workers = raw.tool_workers?.[schema.key] ?? [];
      return [{
        category,
        provider: schema.key,
        name: schema.name,
        state: schema.licensed ? 'ready' : 'not_licensed',
        reason: schema.licensed ? undefined : schema.license_message ?? undefined,
        version: schema.cli_version,
        schema,
        this_server: raw.this_server ?? workers.length === 0,
        workers,
      }];
    });
  return { ...raw, tools, providers };
}

/** Every schema a chain's steps are checked against before a run. */
export function providerSchemas(
  capabilities: ProcessingCapabilities | undefined,
): ExternalToolSchema[] | undefined {
  if (!capabilities) return undefined;
  const schemas = capabilities.providers.flatMap((provider) => provider.schema ? [provider.schema] : []);
  const unlisted = capabilities.tools.filter(
    (tool) => !schemas.some((schema) => schema.key === tool.key)
  );
  return [...schemas, ...unlisted];
}

/** Whether the server lists any of these providers; any at all when
 *  `keys` is absent. */
export function listsProvider(
  capabilities: ProcessingCapabilities | undefined,
  keys?: readonly string[],
): boolean {
  return !!capabilities?.providers.some((provider) => !keys || keys.includes(provider.provider));
}
