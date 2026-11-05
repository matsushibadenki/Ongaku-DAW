import { useEffect, useRef, useState } from "react";
import { LocaleSwitch } from "./LocaleSwitch.jsx";
import { MetricsPanel } from "./MetricsPanel.jsx";

export function AppMenu({ t, locale, onLocaleChange, session }) {
  const menuRef = useRef(null);
  const [activeMenu, setActiveMenu] = useState(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const dismiss = (event) => {
      if (event.type === "keydown" ? event.key === "Escape" : !menuRef.current?.contains(event.target)) {
        setActiveMenu(null); setOpen(false);
      }
    };
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", dismiss);
    return () => { document.removeEventListener("pointerdown", dismiss); document.removeEventListener("keydown", dismiss); };
  }, []);
  const { devices, deviceId, setDeviceId, running, busy } = session;
  const openProject = () => {
    if (!session.documentStatus.dirty || window.confirm(t.discardChanges)) session.loadProject();
    setActiveMenu(null);
  };
  const run = (action) => { setActiveMenu(null); action(); };
  return (
    <nav ref={menuRef} className="app-menu" aria-label={t.applicationMenu}>
      <div className="app-menu-brand">
        <button className="app-menu-icon" type="button" aria-label={t.applicationMenu}
          aria-expanded={open} onClick={() => setOpen((current) => !current)}>O</button>
        {open && <div className="app-preferences" role="dialog" aria-label={t.preferences}>
          <strong>{t.preferences}</strong>
          <label>
            <span>{t.audioOutput}</span>
            <select value={deviceId} onChange={(event) => setDeviceId(event.target.value)} disabled={running || busy}>
              <option value="">{t.defaultDevice}</option>
              {devices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
            </select>
          </label>
          <LocaleSwitch label={t.language} locale={locale} onChange={onLocaleChange} />
          <details className="performance-details">
            <summary>{t.performance}</summary>
            <MetricsPanel t={t} metrics={session.metrics} running={running} />
          </details>
        </div>}
      </div>
      <div className="app-menu-items">
        <div className="document-menu">
          <button type="button" aria-expanded={activeMenu === "file"} onClick={() => setActiveMenu(activeMenu === "file" ? null : "file")}>{t.menuFile}</button>
          {activeMenu === "file" && <div className="document-menu-popup">
            <button type="button" disabled={busy} onClick={openProject}>{t.openProject}<kbd>⌘/Ctrl O</kbd></button>
            <button type="button" disabled={busy} onClick={() => run(() => session.saveProject())}>{t.saveProject}<kbd>⌘/Ctrl S</kbd></button>
            {!session.previewMode && <button type="button" disabled={busy} onClick={() => run(() => session.saveProject(true))}>{t.saveAs}</button>}
          </div>}
        </div>
        <div className="document-menu">
          <button type="button" aria-expanded={activeMenu === "edit"} onClick={() => setActiveMenu(activeMenu === "edit" ? null : "edit")}>{t.menuEdit}</button>
          {activeMenu === "edit" && <div className="document-menu-popup">
            <button type="button" disabled={busy || !session.documentStatus.canUndo} onClick={() => run(session.undo)}>{t.undo}<kbd>⌘/Ctrl Z</kbd></button>
            <button type="button" disabled={busy || !session.documentStatus.canRedo} onClick={() => run(session.redo)}>{t.redo}<kbd>⇧⌘/Ctrl Z</kbd></button>
          </div>}
        </div>
        <span className="document-indicator" title={session.previewMode ? t.previewStorage : session.documentStatus.path ?? t.unsaved}>
          {session.documentStatus.dirty ? "● " : ""}{session.project?.name}
        </span>
      </div>
    </nav>
  );
}
