import type { StackDeconvolutionConfig } from '../api/types';

export function defaultDeconvolutionConfig(): StackDeconvolutionConfig {
  return {
    psf_fwhm_pixels: 3.1,
    iterations: 4,
    amount: 0.35,
    noise_fraction: 0.001,
    max_correction: 2,
  };
}

export function validateDeconvolution(
  config: StackDeconvolutionConfig | null | undefined
): string | null {
  if (!config) return null;
  if (!Number.isFinite(config.psf_fwhm_pixels) ||
      config.psf_fwhm_pixels < 0.25 || config.psf_fwhm_pixels > 100) {
    return 'PSF FWHM must be between 0.25 and 100 pixels';
  }
  if (!Number.isInteger(config.iterations) || config.iterations < 1 || config.iterations > 50) {
    return 'Deconvolution iterations must be an integer from 1 to 50';
  }
  if (!Number.isFinite(config.amount) || config.amount < 0 || config.amount > 1) {
    return 'Deconvolution amount must be between 0 and 1';
  }
  if (!Number.isFinite(config.noise_fraction) ||
      config.noise_fraction < 0 || config.noise_fraction > 0.25) {
    return 'Noise fraction must be between 0 and 0.25';
  }
  if (!Number.isFinite(config.max_correction) ||
      config.max_correction < 1 || config.max_correction > 100) {
    return 'Maximum correction must be between 1 and 100';
  }
  return null;
}
