import { isAxiosError } from 'axios';

/** Director writes take turns at the metadata store; one that waited its full turn answers 503. Wait, do not fail. */
export function retryWhenBusy(count: number, error: Error): boolean {
  const status = isAxiosError(error) ? error.response?.status
    : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
  return status === 503 && count < 5;
}
