import { useState } from 'react';
import { isAxiosError } from 'axios';
import { Download, Trash2, Upload } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorHorizon } from '../../api/directorTypes';
import { describeHorizon, downloadHrz } from './horizonFile';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The horizon file could not be read';

/** Paste or upload a N.I.N.A. `.hrz` file, download it back, or clear it.
 *  The server reads the file, so the curve matches what planning checks. */
export default function HorizonEditor({ label, name, horizon, note, disabled, onChange }: {
  label: string;
  /** Names the downloaded file. */
  name: string;
  horizon: DirectorHorizon | null;
  /** Where the current curve came from, shown after its summary. */
  note?: string;
  disabled: boolean;
  onChange: (horizon: DirectorHorizon | null) => void;
}) {
  const [text, setText] = useState('');
  const [problem, setProblem] = useState('');
  const [reading, setReading] = useState(false);
  const read = async (source: string) => {
    setProblem('');
    setReading(true);
    try {
      onChange(await apiClient.parseDirectorHorizon(source));
      setText('');
    } catch (error) {
      setProblem(message(error));
    } finally {
      setReading(false);
    }
  };
  const custom = horizon?.mode === 'custom' ? horizon : null;
  return <div className="horizon-editor" role="group" aria-label={label}>
    <p className="director-muted" data-testid="horizon-summary">Horizon: {describeHorizon(horizon)}{note ? `, ${note}` : ''}.</p>
    {!disabled && <>
      <label className="horizon-paste">Paste a N.I.N.A. horizon file
        <textarea aria-label={`${label} file text`} rows={4} spellCheck={false} placeholder={'0 12\n90 20\n180 15\n270 18\n'} value={text} onChange={event => setText(event.target.value)} />
      </label>
      <div className="director-actions">
        <button type="button" disabled={!text.trim() || reading} onClick={() => void read(text)}>{reading ? 'Reading...' : 'Use pasted horizon'}</button>
        <label className="horizon-upload"><Upload size={16} />Upload .hrz
          <input type="file" accept=".hrz,.txt,text/plain" aria-label={`Upload ${label} file`} onChange={event => {
            const file = event.target.files?.[0];
            event.target.value = '';
            if (file) void file.text().then(read);
          }} />
        </label>
        {custom && <button type="button" onClick={() => downloadHrz(name, custom)}><Download size={16} />Download .hrz</button>}
        {horizon && <button type="button" onClick={() => { setProblem(''); onChange(null); }}><Trash2 size={16} />Clear horizon</button>}
      </div>
    </>}
    {disabled && custom && <div className="director-actions"><button type="button" onClick={() => downloadHrz(name, custom)}><Download size={16} />Download .hrz</button></div>}
    {problem && <p className="director-error" role="alert">{problem}</p>}
  </div>;
}
