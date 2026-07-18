import { suggestedDeepSkyColorForObject } from '@seiza/astro-overlay';
import {
  AstroOverlay,
  type AstroOverlayProps,
} from '@seiza/astro-overlay/react';

/**
 * PSF Guard's shared-overlay policy: retain Seiza's generic renderer and opt
 * into its catalog-aware deep-sky palette. Callers can still supply a custom
 * resolver when a future preferences UI exposes one.
 */
export default function AstrometryOverlay({
  colorForObject = suggestedDeepSkyColorForObject,
  ...props
}: AstroOverlayProps) {
  return <AstroOverlay colorForObject={colorForObject} {...props} />;
}
