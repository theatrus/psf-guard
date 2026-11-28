import { describe, expect, it } from 'vitest';
import { studioLink } from '../studioLink';

describe('the link that opens a target in PSF Studio', () => {
  it('names the server, the database, the project and the target', () => {
    expect(studioLink('http://localhost:3000', 'my db', 11, 17)).toBe(
      'psf-studio://open?server=http%3A%2F%2Flocalhost%3A3000&db=my+db&project=11&target=17',
    );
  });

  it('carries a ticket on a server with sign-in', () => {
    expect(studioLink('https://guard.example', 'main', 1, 2, 'ab12')).toBe(
      'psf-studio://open?server=https%3A%2F%2Fguard.example&db=main&project=1&target=2&ticket=ab12',
    );
  });
});
