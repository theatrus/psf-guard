import { useCallback } from 'react';
import { useNavigate, type NavigateOptions, type To } from 'react-router-dom';

/**
 * Navigate and draw the new view before the browser handles another event.
 *
 * React Router draws a route change in a transition: the URL changes at
 * once, but React may draw the view it names several frames later. A key
 * pressed in between finds the old view still listening. The grid ignores
 * it, so the key is lost; the detail or Sequence being closed acts on it.
 *
 * Moves that hand the keyboard to another view (opening, stepping, and
 * closing the image detail and Compare) go through this hook, so the URL
 * and the view listening for keys change in the same step.
 */
export function useNavigateNow() {
  const navigate = useNavigate();
  return useCallback(
    (to: To, options?: NavigateOptions) => {
      void navigate(to, { ...options, flushSync: true });
    },
    [navigate],
  );
}
