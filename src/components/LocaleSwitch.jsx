import { languages } from "../i18n.js";

export function LocaleSwitch({ label, locale, onChange }) {
  return (
    <label className="locale-switch"><span>{label}</span><select value={locale} onChange={(event) => onChange(event.target.value)}>
      {languages.map((language) => <option key={language.code} value={language.code}>{language.label}</option>)}
    </select></label>
  );
}
