export function EngineToolbar({ t, running, busy, deviceId, devices, onDeviceChange, onToggle }) {
  const state = busy ? "loading" : running ? "success" : "default";
  return (
    <section className="toolbar" aria-label={t.device}>
      <label className="field">
        <span>{t.device}</span>
        <select value={deviceId} onChange={(event) => onDeviceChange(event.target.value)} disabled={running || busy}>
          <option value="">{t.defaultDevice}</option>
          {devices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
        </select>
      </label>
      <button className="transport-button" type="button" data-state={state} disabled={busy} onClick={onToggle}>
        <span className="transport-mark" aria-hidden="true">{running ? "■" : "▶"}</span>
        {busy ? "…" : running ? t.stop : t.start}
      </button>
    </section>
  );
}
