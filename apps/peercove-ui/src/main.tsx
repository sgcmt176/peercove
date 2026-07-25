import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, loadTheme } from "./theme";
import "./styles.css";

// 最初の描画前にテーマを適用する(App の useEffect 任せだと、ダーク設定時に
// 一瞬ライトで描かれてちらつく)
applyTheme(loadTheme());

// WebView 既定の右クリックメニュー(「戻る」「印刷」等のブラウザ由来の
// メニュー)をアプリ全体で抑止する(M6 H-9 追補、依頼者指摘)。独自の
// ContextMenu を出す箇所は各コンポーネントの onContextMenu が処理し、
// それ以外の場所では何も出さない。ただしテキスト入力欄(input/textarea)
// だけは OS 標準の切り取り/コピー/貼り付けメニューを残す(右クリック
// 貼り付けの操作を奪わないため)。
document.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement | null;
  if (target?.closest("input, textarea, [contenteditable='true']")) return;
  event.preventDefault();
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
