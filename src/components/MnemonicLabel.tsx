/** Label with its shortcut letter underlined, e.g. "Full _p_ath". Use for
 *  keys that are a letter of the label; use a trailing `<kbd>` otherwise. */
export function MnemonicLabel({ text, mnemonic }: { text: string; mnemonic: string }) {
  const idx = text.toLowerCase().indexOf(mnemonic.toLowerCase());
  if (idx < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <span className="modal__mnemonic">{text[idx]}</span>
      {text.slice(idx + 1)}
    </>
  );
}
