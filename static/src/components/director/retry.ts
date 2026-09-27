import { isAxiosError } from 'axios';

/** Director admits one metadata request at a time and answers 503 while busy; wait, do not fail. */
export function retryWhenBusy(count: number, error: Error): boolean {
  const status = isAxiosError(error) ? error.response?.status
    : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
  return status === 503 && count < 5;
}
