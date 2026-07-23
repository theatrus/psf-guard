import type { SiteBanner as SiteBannerConfig } from '../api/types';

interface SiteBannerProps {
  banner?: SiteBannerConfig;
}

export default function SiteBanner({ banner }: SiteBannerProps) {
  if (!banner) return null;

  const showLink = Boolean(banner.link_text && banner.link_url);
  return (
    <aside className="site-banner" aria-label={banner.title}>
      <strong className="site-banner__title">{banner.title}</strong>
      <span className="site-banner__message">{banner.message}</span>
      {showLink && (
        <a
          className="site-banner__link"
          href={banner.link_url}
          target="_blank"
          rel="noreferrer"
        >
          {banner.link_text} <span aria-hidden="true">↗</span>
        </a>
      )}
    </aside>
  );
}
