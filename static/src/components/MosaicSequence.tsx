import { useMemo, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import type { Image, ProjectMosaic } from '../api/types';
import ImageCard from './ImageCard';
import { imageDetailPath } from '../utils/imageDetailRoutes';
import { mosaicLabel } from '../hooks/useProjectMosaic';

interface MosaicSequenceProps {
  dbId: string;
  mosaic: ProjectMosaic;
  images: Image[];
}

/** A mosaic's frames in capture order, every panel's interleaved and each
 *  card named by its panel. Scoring stays per panel: each panel's own
 *  sequence is one click away. */
export default function MosaicSequence({ dbId, mosaic, images }: MosaicSequenceProps) {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const filterName = searchParams.get('filterName') || undefined;
  const [selectedId, setSelectedId] = useState<number | null>(null);

  const panelOf = useMemo(
    () => new Map(mosaic.panels.map(panel => [panel.target_id, panel])),
    [mosaic],
  );
  const filters = useMemo(
    () => [...new Set(images.filter(image => panelOf.has(image.target_id)).flatMap(image => image.filter_name ? [image.filter_name] : []))].sort(),
    [images, panelOf],
  );
  const frames = useMemo(
    () => images
      .filter(image => panelOf.has(image.target_id) && (!filterName || image.filter_name === filterName))
      .sort((a, b) => (a.acquired_date ?? 0) - (b.acquired_date ?? 0) || a.id - b.id),
    [images, panelOf, filterName],
  );

  const withParams = (change: (params: URLSearchParams) => void) => {
    const params = new URLSearchParams(searchParams);
    change(params);
    navigate(`/sequence?${params}`);
  };
  const openPanel = (targetId: number) => withParams(params => {
    params.set('target', String(targetId));
    params.delete('mosaic');
  });

  return (
    <div className="sequence-view mosaic-sequence">
      <div className="sequence-header">
        <h2>{mosaicLabel(mosaic)}</h2>
        <p className="mosaic-sequence-note">
          Every panel's frames in capture order. Scores come from each panel's sequence.
        </p>
        <div className="mosaic-sequence-controls">
          {filters.length > 1 && (
            <label className="filter-input-group">
              Filter:
              <select
                value={filterName ?? 'all'}
                onChange={event => withParams(params => {
                  if (event.target.value === 'all') params.delete('filterName');
                  else params.set('filterName', event.target.value);
                })}
              >
                <option value="all">All</option>
                {filters.map(filter => <option key={filter} value={filter}>{filter}</option>)}
              </select>
            </label>
          )}
          <div className="mosaic-sequence-panels" role="group" aria-label="Open a panel's sequence">
            {mosaic.panels.map(panel => (
              <button key={panel.target_id} type="button" className="target-card-btn" title={panel.target_name}
                onClick={() => openPanel(panel.target_id)}>
                <span className="target-card-name">{panel.panel_id}</span>
                <span className="target-card-count">{panel.target_name}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
      {frames.length === 0 ? (
        <div className="empty-state">No frames for this mosaic yet.</div>
      ) : (
        <ol className="mosaic-sequence-frames" aria-label="Frames in capture order">
          {frames.map(image => {
            const panel = panelOf.get(image.target_id)!;
            return (
              <li key={image.id} data-image-id={image.id} className="mosaic-sequence-frame">
                <span className="mosaic-sequence-panel" title={panel.target_name}>{panel.panel_id}</span>
                <ImageCard
                  dbId={dbId}
                  image={image}
                  isSelected={image.id === selectedId}
                  qualityPresentation="compact"
                  lazyPreview
                  onClick={() => setSelectedId(image.id)}
                  onDoubleClick={() => navigate(imageDetailPath(image.id, searchParams, 'sequence'))}
                />
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
