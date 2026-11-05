import { Icon } from "./Icons.jsx";

export function WorkspaceToolbar({ t, selectedTrack, inspectorOpen, editorOpen, browserOpen, onToggleBrowser, onToggleInspector, onToggleEditor }) {
  return (
    <div className="workspace-toolbar" role="toolbar" aria-label={t.workspaceViews}>
      <div className="workspace-toolbar-title">
        <span>{t.timeline}</span>
        {selectedTrack && <strong>{t.trackRoles[selectedTrack.role]}</strong>}
      </div>
      <div className="workspace-view-actions">
        <button type="button" aria-label={t.inspector} title={t.inspector} aria-pressed={inspectorOpen} onClick={onToggleInspector}><Icon name="panel" /></button>
        <button type="button" aria-label={t.detailEditor} title={t.detailEditor} aria-pressed={editorOpen} onClick={onToggleEditor}><Icon name="editor" /></button>
        <button type="button" aria-label={t.browser} title={t.browser} aria-pressed={browserOpen} onClick={onToggleBrowser}><Icon name="browser" /></button>
      </div>
    </div>
  );
}
