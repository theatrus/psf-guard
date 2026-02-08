import { describe, it, expect } from 'vitest';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import cloudsFixture from '../../__fixtures__/sequence-analysis-clouds.json';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';
import imageQualityFixture from '../../__fixtures__/image-quality-context.json';
import emptyFixture from '../../__fixtures__/sequence-analysis-empty.json';
import type {
  SequenceAnalysisResponse,
  ImageQualityResponse,
  ScoredSequence,
  ImageQualityResult,
  ReferenceValues,
  SequenceSummary,
} from '../types';

// Helper to check ApiResponse wrapper structure
function assertApiResponseWrapper(fixture: Record<string, unknown>) {
  expect(fixture).toHaveProperty('success');
  expect(fixture).toHaveProperty('data');
  expect(fixture).toHaveProperty('error');
  expect(fixture).toHaveProperty('status');
  expect(typeof fixture.success).toBe('boolean');
}

// Helper to validate a ScoredSequence object has all expected fields
function assertScoredSequence(seq: ScoredSequence) {
  expect(typeof seq.target_id).toBe('number');
  expect(typeof seq.target_name).toBe('string');
  expect(typeof seq.filter_name).toBe('string');
  expect(typeof seq.image_count).toBe('number');
  expect(seq).toHaveProperty('session_start');
  expect(seq).toHaveProperty('session_end');
  expect(seq).toHaveProperty('reference_values');
  expect(seq).toHaveProperty('images');
  expect(seq).toHaveProperty('summary');
  expect(Array.isArray(seq.images)).toBe(true);
}

// Helper to validate ImageQualityResult fields
function assertImageQualityResult(img: ImageQualityResult) {
  expect(typeof img.image_id).toBe('number');
  expect(typeof img.quality_score).toBe('number');
  expect(img.quality_score).toBeGreaterThanOrEqual(0);
  expect(img.quality_score).toBeLessThanOrEqual(1);
  expect(typeof img.temporal_anomaly_score).toBe('number');
  expect(img).toHaveProperty('category');
  expect(img).toHaveProperty('normalized_metrics');
  expect(img).toHaveProperty('details');

  // Validate normalized_metrics has all expected keys
  const nm = img.normalized_metrics;
  expect(nm).toHaveProperty('star_count');
  expect(nm).toHaveProperty('hfr');
  expect(nm).toHaveProperty('eccentricity');
  expect(nm).toHaveProperty('snr');
  expect(nm).toHaveProperty('background');
}

// Helper to validate ReferenceValues fields
function assertReferenceValues(refs: ReferenceValues) {
  expect(refs).toHaveProperty('best_star_count');
  expect(refs).toHaveProperty('best_hfr');
  expect(refs).toHaveProperty('best_eccentricity');
  expect(refs).toHaveProperty('best_snr');
  expect(refs).toHaveProperty('best_background');
}

// Helper to validate SequenceSummary fields
function assertSequenceSummary(summary: SequenceSummary) {
  expect(typeof summary.excellent_count).toBe('number');
  expect(typeof summary.good_count).toBe('number');
  expect(typeof summary.fair_count).toBe('number');
  expect(typeof summary.poor_count).toBe('number');
  expect(typeof summary.bad_count).toBe('number');
  expect(typeof summary.cloud_events_detected).toBe('number');
  expect(typeof summary.focus_drift_detected).toBe('boolean');
  expect(typeof summary.tracking_issues_detected).toBe('boolean');
}

describe('API contract: ApiResponse wrapper', () => {
  it.each([
    ['normal', normalFixture],
    ['clouds', cloudsFixture],
    ['multi-session', multiSessionFixture],
    ['image-quality', imageQualityFixture],
    ['empty', emptyFixture],
  ])('%s fixture has correct wrapper structure', (_name, fixture) => {
    assertApiResponseWrapper(fixture);
  });

  it('successful responses have success=true and null error', () => {
    expect(normalFixture.success).toBe(true);
    expect(normalFixture.error).toBeNull();
    expect(normalFixture.status).toBe('ready');
  });
});

describe('API contract: SequenceAnalysisResponse', () => {
  it('normal fixture matches SequenceAnalysisResponse shape', () => {
    const data = normalFixture.data as SequenceAnalysisResponse;
    expect(data).toHaveProperty('sequences');
    expect(Array.isArray(data.sequences)).toBe(true);
    expect(data.sequences).toHaveLength(1);

    const seq = data.sequences[0];
    assertScoredSequence(seq);
    assertReferenceValues(seq.reference_values);
    assertSequenceSummary(seq.summary);
    seq.images.forEach(assertImageQualityResult);
  });

  it('summary counts sum to image_count', () => {
    const seq = normalFixture.data.sequences[0];
    const { excellent_count, good_count, fair_count, poor_count, bad_count } = seq.summary;
    const total = excellent_count + good_count + fair_count + poor_count + bad_count;
    expect(total).toBe(seq.image_count);
  });

  it('images array length matches image_count', () => {
    const seq = normalFixture.data.sequences[0];
    expect(seq.images).toHaveLength(seq.image_count);
  });
});

describe('API contract: cloud detection', () => {
  it('clouds fixture has likely_clouds category values', () => {
    const seq = cloudsFixture.data.sequences[0];
    const cloudImages = seq.images.filter(
      (img: ImageQualityResult) => img.category === 'likely_clouds',
    );
    expect(cloudImages.length).toBeGreaterThanOrEqual(1);
  });

  it('category values use snake_case', () => {
    const seq = cloudsFixture.data.sequences[0];
    seq.images.forEach((img: ImageQualityResult) => {
      if (img.category !== null) {
        expect(img.category).toMatch(/^[a-z][a-z0-9_]*$/);
      }
    });
  });

  it('cloud-affected images have quality_score < 0.3', () => {
    const seq = cloudsFixture.data.sequences[0];
    const cloudImages = seq.images.filter(
      (img: ImageQualityResult) => img.category === 'likely_clouds',
    );
    cloudImages.forEach((img: ImageQualityResult) => {
      expect(img.quality_score).toBeLessThan(0.3);
    });
  });

  it('summary reports cloud events', () => {
    const seq = cloudsFixture.data.sequences[0];
    expect(seq.summary.cloud_events_detected).toBeGreaterThanOrEqual(1);
  });
});

describe('API contract: multi-session', () => {
  it('multi-session fixture has 2 sequences', () => {
    const data = multiSessionFixture.data as SequenceAnalysisResponse;
    expect(data.sequences).toHaveLength(2);
  });

  it('session 1 ends before session 2 starts', () => {
    const [seq1, seq2] = multiSessionFixture.data.sequences;
    expect(seq1.session_end).toBeDefined();
    expect(seq2.session_start).toBeDefined();
    expect(seq1.session_end!).toBeLessThan(seq2.session_start!);
  });

  it('both sequences share the same target', () => {
    const [seq1, seq2] = multiSessionFixture.data.sequences;
    expect(seq1.target_id).toBe(seq2.target_id);
    expect(seq1.target_name).toBe(seq2.target_name);
  });

  it('each sequence has correct image_count', () => {
    multiSessionFixture.data.sequences.forEach((seq) => {
      assertScoredSequence(seq as ScoredSequence);
      expect(seq.images).toHaveLength(seq.image_count);
    });
  });
});

describe('API contract: ImageQualityResponse', () => {
  it('image quality fixture matches ImageQualityResponse shape', () => {
    const data = imageQualityFixture.data as ImageQualityResponse;
    expect(typeof data.image_id).toBe('number');
    expect(data).toHaveProperty('quality');
    expect(data).toHaveProperty('sequence_target_id');
    expect(data).toHaveProperty('sequence_filter_name');
    expect(data).toHaveProperty('sequence_image_count');
    expect(data).toHaveProperty('reference_values');
  });

  it('quality field contains valid ImageQualityResult', () => {
    const data = imageQualityFixture.data as ImageQualityResponse;
    expect(data.quality).toBeDefined();
    assertImageQualityResult(data.quality!);
  });

  it('reference_values contains all expected fields', () => {
    const data = imageQualityFixture.data as ImageQualityResponse;
    expect(data.reference_values).toBeDefined();
    assertReferenceValues(data.reference_values!);
  });
});

describe('API contract: empty response', () => {
  it('empty fixture has empty sequences array', () => {
    const data = emptyFixture.data as SequenceAnalysisResponse;
    expect(data.sequences).toHaveLength(0);
  });

  it('empty fixture still has valid wrapper', () => {
    assertApiResponseWrapper(emptyFixture);
    expect(emptyFixture.success).toBe(true);
  });
});

describe('API contract: normalized metric values', () => {
  it('numeric normalized metrics are between 0 and 1', () => {
    normalFixture.data.sequences[0].images.forEach((img: ImageQualityResult) => {
      const nm = img.normalized_metrics;
      for (const [, value] of Object.entries(nm)) {
        if (value !== null && value !== undefined) {
          expect(value).toBeGreaterThanOrEqual(0);
          expect(value).toBeLessThanOrEqual(1);
        }
      }
    });
  });

  it('nullable metrics can be null', () => {
    // Image 10 in the normal fixture has snr: null
    const img10 = normalFixture.data.sequences[0].images[9];
    expect(img10.normalized_metrics.snr).toBeNull();
  });
});
