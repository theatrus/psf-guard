import { useEffect, useState } from 'react';

/**
 * What the image viewers report under the pointer: the pixel's linear value
 * and its sky position. One choice for the image reviewer and the stack
 * inspector alike, remembered across images and reloads. Both start off:
 * the readout is for someone who turned it on.
 */
export interface MeasureOptions {
  value: boolean;
  sky: boolean;
}

const STORAGE_KEY = 'psf-guard.measure-options';

type Listener = (options: MeasureOptions) => void;

const listeners = new Set<Listener>();

function readStored(): MeasureOptions {
  try {
    const stored = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? '{}');
    return { value: stored?.value === true, sky: stored?.sky === true };
  } catch {
    // Storage can be refused (private browsing); the readout then starts off.
    return { value: false, sky: false };
  }
}

let options = readStored();

export function setMeasureOption(option: keyof MeasureOptions, enabled: boolean): void {
  if (options[option] === enabled) return;
  options = { ...options, [option]: enabled };
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(options));
  } catch {
    // Keep the in-memory choice even when it cannot be persisted.
  }
  listeners.forEach((listener) => listener(options));
}

export function toggleMeasureOption(option: keyof MeasureOptions): void {
  setMeasureOption(option, !options[option]);
}

/** Subscribe to the shared choice. Returns its current value. */
export function useMeasureOptions(): MeasureOptions {
  const [value, setValue] = useState(options);
  useEffect(() => {
    listeners.add(setValue);
    setValue(options);
    return () => {
      listeners.delete(setValue);
    };
  }, []);
  return value;
}
