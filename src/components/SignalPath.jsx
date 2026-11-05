export function SignalPath({ t, running, config }) {
  const nodes = [
    [t.oscillator, t.oscillatorMeta],
    [t.gain, "Atomic<f32>"],
    [t.master, "f32 · interleaved"],
  ];
  return (
    <section className="panel signal-panel" aria-labelledby="signal-title">
      <header className="panel-header"><h2 id="signal-title">{t.signalPath}</h2><span className="format">{new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(config.sampleRate / 1_000)} kHz / {config.bufferSize}</span></header>
      <div className="signal-path">
        {nodes.map(([title, meta], index) => (
          <div className="signal-step" key={title}>
            <div className="signal-node"><strong>{title}</strong><span>{meta}</span></div>
            {index < nodes.length - 1 && <span className={running ? "connector is-live" : "connector"} aria-hidden="true">→</span>}
          </div>
        ))}
      </div>
    </section>
  );
}
