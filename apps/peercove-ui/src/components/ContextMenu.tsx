// 汎用の右クリックメニュー部品(M6 H-5、ADR-0055 決定 6)。まずシート
// (SheetView)で採用し、他画面へは順次展開する。位置指定・項目リスト・
// 区切り・無効項目・Esc/外側クリックで閉じる。ダークモード対応は CSS 側
// (var(--surface) 等のテーマ変数)で行う。
import { useEffect, useRef } from "react";

export interface ContextMenuItem {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  /** 削除など破壊的操作の強調表示。 */
  danger?: boolean;
}

export type ContextMenuEntry = ContextMenuItem | { separator: true };

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  /** 画面座標(px)。ビューポート内に収まるよう自動で補正する。 */
  x: number;
  y: number;
  items: ContextMenuEntry[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    const onPointerDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    // click ではなく mousedown: SheetView 側の「外側クリックで閉じる」系の
    // document click ハンドラより先に判定を終えたい(open 直後の右クリック
    // が click イベントとして即座に自分自身を閉じてしまうのを避ける)。
    document.addEventListener("mousedown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [onClose]);

  // ビューポート外へはみ出さないよう補正(初回レンダー後の実測サイズで調整)
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const overflowX = rect.right - window.innerWidth;
    const overflowY = rect.bottom - window.innerHeight;
    if (overflowX > 0) el.style.left = `${Math.max(0, x - overflowX)}px`;
    if (overflowY > 0) el.style.top = `${Math.max(0, y - overflowY)}px`;
  }, [x, y]);

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      style={{ left: x, top: y }}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item, index) =>
        "separator" in item ? (
          <div key={index} className="context-menu__sep" role="separator" />
        ) : (
          <button
            key={index}
            type="button"
            role="menuitem"
            className={
              item.danger
                ? "context-menu__item context-menu__item--danger"
                : "context-menu__item"
            }
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.onClick();
            }}
          >
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}
