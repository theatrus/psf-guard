import type { WbppOptions, WbppRejection } from '../api/types';
import './WbppOptionsFields.css';

interface Props {
  value: WbppOptions;
  onChange: (next: WbppOptions) => void;
  /** Distinguishes several instances on one page. */
  idPrefix?: string;
  disabled?: boolean;
}

const REJECTIONS: Array<{ value: WbppRejection | ''; label: string }> = [
  { value: '', label: "WBPP's default (automatic)" },
  { value: 'auto', label: 'Automatic' },
  { value: 'winsorized_sigma', label: 'Winsorized sigma clipping' },
  { value: 'linear_fit', label: 'Linear fit clipping' },
  { value: 'esd', label: 'Generalized ESD' },
  { value: 'rcr', label: 'Robust Chauvenet' },
  { value: 'percentile_clip', label: 'Percentile clipping' },
];

/**
 * The settings a WBPP run takes beyond its frames: what its Presets
 * dialog and its per-group controls set, the ones a headless run cannot
 * be handed afterwards.
 */
export default function WbppOptionsFields({ value, onChange, idPrefix = 'wbpp', disabled }: Props) {
  const set = <K extends keyof WbppOptions>(key: K, next: WbppOptions[K]) =>
    onChange({ ...value, [key]: next });
  const id = (name: string) => `${idPrefix}-${name}`;

  return (
    <div className="wbpp-options">
      <label className="wbpp-option" htmlFor={id('quality')}>
        <span>
          Quality
          <small>
            WBPP&apos;s own presets. Maximum: local normalization on, PSF Auto, every star.
            Good: local normalization on, Moffat 4, 500 stars. Fast: no local normalization.
          </small>
        </span>
        <select
          id={id('quality')}
          value={value.quality}
          disabled={disabled}
          onChange={(event) => set('quality', event.target.value as WbppOptions['quality'])}
        >
          <option value="maximum">Maximum</option>
          <option value="good">Good</option>
          <option value="fast">Fast</option>
        </select>
      </label>
      <label className="wbpp-option" htmlFor={id('fast-integration')}>
        <span>
          Fast Integration
          <small>
            In-memory stacking with less weighting. WBPP switches any group of 150 or more
            frames to it on its own unless told off.
          </small>
        </span>
        <select
          id={id('fast-integration')}
          value={value.fast_integration}
          disabled={disabled}
          onChange={(event) =>
            set('fast_integration', event.target.value as WbppOptions['fast_integration'])
          }
        >
          <option value="off">Off for every group</option>
          <option value="auto">WBPP decides</option>
          <option value="on">On for every group</option>
        </select>
      </label>
      <label className="wbpp-option" htmlFor={id('drizzle')}>
        <span>
          Drizzle
          <small>Drizzle integration of the lights, at WBPP&apos;s default drop shrink.</small>
        </span>
        <select
          id={id('drizzle')}
          value={value.drizzle}
          disabled={disabled}
          onChange={(event) => set('drizzle', event.target.value as WbppOptions['drizzle'])}
        >
          <option value="off">Off</option>
          <option value="2x">2x</option>
          <option value="3x">3x</option>
        </select>
      </label>
      <label className="wbpp-option" htmlFor={id('autocrop')}>
        <span>
          Autocrop
          <small>Crop the masters to the area every frame covers.</small>
        </span>
        <select
          id={id('autocrop')}
          value={value.autocrop === undefined ? '' : value.autocrop ? 'on' : 'off'}
          disabled={disabled}
          onChange={(event) =>
            set(
              'autocrop',
              event.target.value === '' ? undefined : event.target.value === 'on'
            )
          }
        >
          <option value="">WBPP&apos;s default (on)</option>
          <option value="on">On</option>
          <option value="off">Off</option>
        </select>
      </label>
      <label className="wbpp-option" htmlFor={id('rejection')}>
        <span>
          Light rejection
          <small>The pixel rejection algorithm for the light integration.</small>
        </span>
        <select
          id={id('rejection')}
          value={value.rejection ?? ''}
          disabled={disabled}
          onChange={(event) =>
            set('rejection', event.target.value === '' ? undefined : (event.target.value as WbppRejection))
          }
        >
          {REJECTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
