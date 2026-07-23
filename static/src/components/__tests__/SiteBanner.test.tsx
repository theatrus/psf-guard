import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import SiteBanner from '../SiteBanner';

describe('SiteBanner', () => {
  it('renders configured plain text and an external link', () => {
    render(
      <SiteBanner
        banner={{
          title: 'Demo site',
          message: 'Sample data; changes may be reset.',
          link_text: 'Learn more',
          link_url: 'https://psf-guard.com/',
        }}
      />,
    );

    expect(
      screen.getByRole('complementary', { name: 'Demo site' }),
    ).toBeInTheDocument();
    expect(screen.getByText('Sample data; changes may be reset.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Learn more/ })).toHaveAttribute(
      'href',
      'https://psf-guard.com/',
    );
  });

  it('does not interpret configured text as markup', () => {
    const { container } = render(
      <SiteBanner
        banner={{
          title: '<em>Demo</em>',
          message: '<script>alert(1)</script>',
        }}
      />,
    );

    expect(screen.getByText('<em>Demo</em>')).toBeInTheDocument();
    expect(screen.getByText('<script>alert(1)</script>')).toBeInTheDocument();
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('em')).toBeNull();
  });

  it('renders nothing when the server has no banner', () => {
    const { container } = render(<SiteBanner />);
    expect(container).toBeEmptyDOMElement();
  });
});
