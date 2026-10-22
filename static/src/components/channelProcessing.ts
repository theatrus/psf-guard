import type {
  RcAstroProcessing,
  StackColorProcessing,
  StackColorRole,
  StackColorSource,
  StackDeconvolutionConfig,
  StackStretchPreview,
} from '../api/types';

/** The query holding the processing a channel card shows for one exact stack
 *  revision. The stacks panel and the color cards share it, so a color card
 *  reads what the channel cards already fetched. */
export function appliedProcessingQueryKey(
  dbId: string,
  projectId: number,
  jobId: string,
  groupIndex: number,
  artifactRevision: string
) {
  return [
    'db', dbId, 'project', projectId, 'stack-processing', `${jobId}:${groupIndex}:${artifactRevision}`,
  ] as const;
}

/** Every color catalog query of a project. A color preview that follows the
 *  channel cards goes out of date when a card's processing changes, so the
 *  stacks panel marks these stale when a card applies or reverts. */
export function colorCatalogQueryPrefix(dbId: string, projectId: number) {
  return ['db', dbId, 'project', projectId, 'stack-color', 'catalog'] as const;
}

/** The pixel processing a channel card shows: its deconvolution and its
 *  RC-Astro steps. Stretches stay with the card; a color input has its own. */
export interface ChannelPixelProcessing {
  deconvolution?: StackDeconvolutionConfig;
  rc_astro?: RcAstroProcessing;
}

export type ChannelProcessing = Partial<Record<StackColorRole, ChannelPixelProcessing>>;

/** Each source's processing, from the channel card's applied preview.
 *  A channel showing no deconvolution and no RC-Astro step is left out. */
export function channelProcessingFrom(
  sources: Pick<StackColorSource, 'role'>[],
  previews: (StackStretchPreview | null | undefined)[]
): ChannelProcessing {
  const result: ChannelProcessing = {};
  sources.forEach((source, index) => {
    const request = previews[index]?.request;
    const deconvolution = request?.deconvolution ?? undefined;
    const rcAstro = request?.rc_astro?.steps.length ? request.rc_astro : undefined;
    if (!deconvolution && !rcAstro) return;
    result[source.role] = {
      ...(deconvolution ? { deconvolution: structuredClone(deconvolution) } : {}),
      ...(rcAstro ? { rc_astro: structuredClone(rcAstro) } : {}),
    };
  });
  return result;
}

/** The draft set to use its own deconvolution and RC-Astro steps, starting
 *  from what each channel card shows, so switching from following the cards
 *  loses nothing. An input whose card shows neither gets none. */
export function ownFromChannels(
  draft: StackColorProcessing,
  channels: ChannelProcessing,
  roles: StackColorRole[]
): StackColorProcessing {
  const input_deconvolutions: StackColorProcessing['input_deconvolutions'] = {};
  const input_rc_astro: NonNullable<StackColorProcessing['input_rc_astro']> = {};
  for (const role of roles) {
    const channel = channels[role];
    if (channel?.deconvolution) input_deconvolutions[role] = structuredClone(channel.deconvolution);
    if (channel?.rc_astro) input_rc_astro[role] = structuredClone(channel.rc_astro);
  }
  // A draft without `input_processing` reads as `own`, as the server reads
  // it; leaving the field out keeps own → channels → own an unchanged draft.
  const next: StackColorProcessing = { ...draft, input_deconvolutions, input_rc_astro };
  delete next.input_processing;
  return next;
}

/** A channel card's deconvolution and RC-Astro steps in words, in the order
 *  they run: `deconvolution, BlurXTerminator`. Null when it shows neither. */
export function channelToolsLabel(
  channel: ChannelPixelProcessing | undefined,
  toolName: (tool: string) => string
): string | null {
  const parts = [
    ...(channel?.deconvolution ? ['deconvolution'] : []),
    ...(channel?.rc_astro?.steps.map((step) => toolName(step.tool)) ?? []),
  ];
  return parts.length > 0 ? parts.join(', ') : null;
}
