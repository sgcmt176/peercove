import { ReactNode, useEffect, useRef } from "react";

/** Esc とバックドロップで閉じられるモーダル。 */
export function Modal({
  title,
  onClose,
  children,
  wide = false,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);

  // フォーカスは開いた瞬間だけ当てる。ここに onClose を依存に入れると、親（App）が
  // 2 秒ごとの状態ポーリングで再レンダーするたびに新しい onClose が渡り、この
  // effect が再実行されて入力欄からフォーカスを奪ってしまう（設定編集で発覚）。
  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="backdrop" onClick={onClose}>
      <div
        className={wide ? "modal modal--wide" : "modal"}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        ref={dialogRef}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal__header">
          <h2>{title}</h2>
          <button
            type="button"
            className="button--icon"
            onClick={onClose}
            aria-label="閉じる"
          >
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** 破壊的操作の確認ダイアログ。 */
export function ConfirmModal({
  title,
  message,
  confirmLabel,
  onConfirm,
  onClose,
  busy,
}: {
  title: string;
  message: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
  busy?: boolean;
}) {
  return (
    <Modal title={title} onClose={onClose}>
      <div className="modal__body">{message}</div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          キャンセル
        </button>
        <button
          type="button"
          className="button--danger"
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? "実行中…" : confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
