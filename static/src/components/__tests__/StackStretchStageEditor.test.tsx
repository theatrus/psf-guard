import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { StackStretchRequest } from '../../api/types';
import StackStretchStageEditor from '../StackStretchStageEditor';
import { defaultStretchRequest } from '../stackStretchModels';

function Editor({ seen, channels = 3 }: { seen: StackStretchRequest[]; channels?: 1 | 3 }) {
  const [request, setRequest] = useState(defaultStretchRequest());
  return (
    <StackStretchStageEditor
      label="Stack"
      channels={channels}
      request={request}
      onChange={(next) => { seen.push(next); setRequest(next); }}
    />
  );
}

describe('StackStretchStageEditor', () => {
  it('offers the median anchor stretch with its defaults and settings', () => {
    const seen: StackStretchRequest[] = [];
    render(<Editor seen={seen} />);
    fireEvent.change(screen.getByLabelText('Stack stretch model'), { target: { value: 'median-anchor' } });
    expect(seen.at(-1)!.model).toEqual({
      type: 'median-anchor',
      target_median: 0.25,
      black_point_sigma: 2.7,
      curves_boost: 0,
      hdr_amount: 0,
      hdr_knee: 0.75,
      normalize: false,
    });
    expect(screen.getByLabelText('Stack Black point σ')).toHaveValue(2.7);
    // The knee shows only once highlights are compressed.
    expect(screen.queryByLabelText('Stack Knee')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Stack Highlight compression'), { target: { value: '0.4' } });
    expect(screen.getByLabelText('Stack Knee')).toHaveValue(0.75);
    // Without black clipping the black point is the image minimum.
    fireEvent.click(screen.getByLabelText('Stack Clip black'));
    expect(seen.at(-1)!.model).toMatchObject({ type: 'median-anchor', black_point_sigma: null, hdr_amount: 0.4 });
    expect(screen.queryByLabelText('Stack Black point σ')).not.toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('Stack Clip black'));
    expect(seen.at(-1)!.model).toMatchObject({ black_point_sigma: 2.7 });
    fireEvent.click(screen.getByLabelText('Stack Normalize'));
    expect(seen.at(-1)!.model).toMatchObject({ normalize: true });
    // The color strategy is unchanged by the model's settings.
    expect(seen.at(-1)!.color_strategy).toBe('linked');
  });

  it('offers the layered stretch and drops the color strategy it cannot use', () => {
    const seen: StackStretchRequest[] = [];
    render(<Editor seen={seen} />);
    fireEvent.change(screen.getByLabelText('Stack stretch color strategy'), { target: { value: 'unlinked' } });
    fireEvent.change(screen.getByLabelText('Stack stretch model'), { target: { value: 'layered' } });
    expect(seen.at(-1)).toEqual({
      model: {
        type: 'layered',
        target_background: 0.15,
        aggressiveness: 0.5,
        dynamic_range_compression: 0,
        scale_separation: 256,
        contrast_recovery: 0,
        saturation_amount: 0,
        saturation_boost: 0.5,
        lightness_mask: true,
      },
      color_strategy: 'linked',
    });
    expect(screen.queryByLabelText('Stack stretch color strategy')).not.toBeInTheDocument();
    // The scale only matters once contrast recovery is on.
    expect(screen.queryByLabelText('Stack Scale (px)')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Stack Contrast recovery'), { target: { value: '0.6' } });
    expect(screen.getByLabelText('Stack Scale (px)')).toHaveValue(256);
    // Saturation's own settings appear once saturation is lifted.
    expect(screen.queryByLabelText('Stack Saturation boost')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Stack Saturation'), { target: { value: '0.3' } });
    fireEvent.click(screen.getByLabelText('Stack Lightness mask'));
    expect(seen.at(-1)!.model).toMatchObject({
      type: 'layered', contrast_recovery: 0.6, saturation_amount: 0.3, lightness_mask: false,
    });
  });

  it('leaves saturation out of a mono layered stretch', () => {
    const seen: StackStretchRequest[] = [];
    render(<Editor seen={seen} channels={1} />);
    fireEvent.change(screen.getByLabelText('Stack stretch model'), { target: { value: 'layered' } });
    expect(screen.getByLabelText('Stack Dynamic range compression')).toHaveValue(0);
    expect(screen.queryByLabelText('Stack Saturation')).not.toBeInTheDocument();
  });
});
