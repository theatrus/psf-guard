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

/** The draft with each channel's deconvolution and RC-Astro steps in its
 *  input. A channel showing neither keeps the draft's own. */
export function withChannelProcessing(
  draft: StackColorProcessing,
  channels: ChannelProcessing,
  roles: StackColorRole[]
): StackColorProcessing {
  const input_deconvolutions = { ...draft.input_deconvolutions };
  const input_rc_astro = { ...draft.input_rc_astro };
  for (const role of roles) {
    const channel = channels[role];
    if (!channel) continue;
    if (channel.deconvolution) input_deconvolutions[role] = structuredClone(channel.deconvolution);
    else delete input_deconvolutions[role];
    if (channel.rc_astro) input_rc_astro[role] = structuredClone(channel.rc_astro);
    else delete input_rc_astro[role];
  }
  return { ...draft, input_deconvolutions, input_rc_astro };
}
