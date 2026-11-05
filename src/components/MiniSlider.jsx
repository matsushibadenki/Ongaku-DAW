export function MiniSlider({ label, value, min, max, step, displayValue, disabled = false, onChange }) {
  const percentage = ((value - min) / (max - min)) * 100;

  return (
    <label className="mini-slider">
      <span className="mini-control-label">{label}</span>
      <span className="mini-slider-track" style={{ "--mini-position": `${percentage}%` }}>
        <span className="mini-slider-fill" style={{ width: `${percentage}%` }} />
        <input type="range" min={min} max={max} step={step} value={value}
          aria-label={label} disabled={disabled}
          onChange={(event) => onChange(Number(event.target.value))} />
      </span>
      {displayValue && <output>{displayValue}</output>}
    </label>
  );
}
