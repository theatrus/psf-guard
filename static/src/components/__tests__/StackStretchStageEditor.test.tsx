import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { StackStretchRequest } from '../../api/types';
import StackStretchStageEditor from '../StackStretchStageEditor';
import { defaultStretchRequest } from '../stackStretchModels';

function Editor({ seen }: { seen: StackStretchRequest[] }) {
  const [request, setRequest] = useState(defaultStretchRequest());
  return (
    <StackStretchStageEditor
      label="Stack"
      channels={3}
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
});
