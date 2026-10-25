import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

/** Open the editor for tests of processing values; flow navigation has its own tests. */
export async function openProcessingFlow() {
  await userEvent.click(screen.getByText('Processing stack'));
  for (const summary of document.querySelectorAll<HTMLElement>('.stack-processing-step > summary, .stack-processing-advanced > summary')) {
    if (!(summary.parentElement as HTMLDetailsElement).open) await userEvent.click(summary);
  }
}
