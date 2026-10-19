import { isAxiosError } from 'axios';

/** The HTTP status a request failed with, through the client's wrapping. */
export function httpStatus(error: unknown): number | undefined {
  if (isAxiosError(error)) return error.response?.status;
  return error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
}
