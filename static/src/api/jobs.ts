/** The query key of the header's job list (`GET /api/jobs`). Code that
 *  starts or changes a job invalidates it, so the header lists the change
 *  without waiting for its next poll. */
export const JOBS_QUERY_KEY = ['jobs'] as const;
