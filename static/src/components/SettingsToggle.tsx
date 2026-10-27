/** On/Off selector shared by calibration and processing settings. */
export default function SettingsToggle({ label, value, disabled, onChange }: {
  label: string;
  value: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <select
      aria-label={label}
      value={value ? 'on' : 'off'}
      disabled={disabled}
      onChange={event => onChange(event.target.value === 'on')}
    >
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
  );
}
