import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, afterAll, beforeAll } from 'vitest';
import { server } from './msw-server';
import { scoringServer } from './scoringDefaults';

// Start MSW server before all tests
beforeAll(() => server.listen({ onUnhandledFrame: 'error' }));

// Reset handlers after each test
afterEach(() => {
  cleanup();
  // What a page keeps for the tab (the Library's filters, for one) must not
  // carry into the next test.
  window.sessionStorage.clear();
  server.resetHandlers();
  scoringServer.reset();
});

// Close server after all tests
afterAll(() => server.close());
