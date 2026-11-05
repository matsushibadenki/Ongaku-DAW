import { MiniSlider } from "./MiniSlider.jsx";

export function GainControl({ t, value, disabled, onChange }) {
  return (
    <label className="gain-control">
      <MiniSlider label={t.gain} min={0} max={2} step={0.01} value={value} disabled={disabled}
        displayValue={value.toFixed(2)} onChange={onChange} />
      <span className="helper">{t.gainHelp}</span>
    </label>
  );
}
