export function MiniVolume({ label, value, min, max, step, displayValue, onChange }) {
  const percentage = ((value - min) / (max - min)) * 100;

  return (
    <label className="mini-volume">
      <span className="mini-control-label">{label}</span>
      <span className="mini-volume-track" style={{ "--mini-position": `${percentage}%` }}>
        <span className="mini-volume-fill" style={{ "--mini-volume-level": `${percentage}%`, width: `${percentage}%` }} />
        <input type="range" min={min} max={max} step={step} value={value}
          aria-label={label}
          onChange={(event) => onChange(Number(event.target.value))} />
      </span>
      <output>{displayValue}</output>
    </label>
  );
}
