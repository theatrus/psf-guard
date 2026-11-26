import { describe, expect, it } from 'vitest';
import { studioLink } from '../studioLink';

describe('the link that opens a target in PSF Studio', () => {
  it('names the server, the database, the project and the target', () => {
    expect(studioLink('http://localhost:3000', 'my db', 11, 17)).toBe(
      'psf-studio://open?server=http%3A%2F%2Flocalhost%3A3000&db=my+db&project=11&target=17',
    );
  });
});
