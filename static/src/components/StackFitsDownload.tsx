import type { ReactNode } from 'react';
import { Download } from 'lucide-react';
import './StackFitsDownload.css';

export default function StackFitsDownload({ href, children }: { href: string; children: ReactNode }) {
  return <a className="stack-fits-download" href={href} download>
    <Download size={14} aria-hidden="true" />
    <span>Download {children}</span>
  </a>;
}
