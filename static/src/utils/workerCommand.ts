const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]', '::1']);

/**
 * The address a worker on another machine uses to reach this server, or a
 * placeholder when this page's address only works on this computer.
 */
export function serverAddress(location: Pick<Location, 'protocol' | 'hostname' | 'host'>): string {
  const reachable = location.protocol.startsWith('http')
    && !LOOPBACK.has(location.hostname)
    && !location.hostname.endsWith('.localhost');
  return reachable ? `${location.protocol}//${location.host}` : 'http://THIS-SERVER:PORT';
}
