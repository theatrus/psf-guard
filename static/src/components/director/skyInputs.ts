/**
 * The key of every save that can change the sky a rig has in view: its
 * location, horizon and altitude limits, its Planning site, a site's
 * location and horizon, and the scheduling minimum time. A plan's Sky tab
 * reads its picture again after one of these, and after no other save.
 */
export const SKY_INPUTS_MUTATION = ['director', 'sky-inputs'] as const;
