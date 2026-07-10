// メンバーの色分けアバター(M3-6)。
//
// 色は公開鍵から決定的に決める(名前変更や並び替えで変わらない・全員の画面で
// 同じ色になる)。彩度と明度は styles.css 側でテーマごとに調整するため、
// ここでは色相(0〜359)だけを CSS 変数として渡す。

/** FNV-1a(32bit)。暗号用途ではなく色相の分散だけが目的。 */
function hueOf(publicKey: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < publicKey.length; i++) {
    hash ^= publicKey.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % 360;
}

export function Avatar({
  publicKey,
  name,
  online,
  onlineLabel,
}: {
  publicKey: string;
  name: string | null;
  online: boolean;
  /** 状態ドットの aria-label(オンライン/オフライン)。 */
  onlineLabel: string;
}) {
  // Intl.Segmenter で書記素単位に切る(絵文字やサロゲートペアの名前でも 1 文字)
  const initial = name
    ? [...new Intl.Segmenter().segment(name.trim())][0]?.segment.toUpperCase() ?? "?"
    : "?";
  return (
    <span
      className={online ? "avatar" : "avatar avatar--offline"}
      style={{ "--hue": hueOf(publicKey) } as React.CSSProperties}
    >
      {initial}
      <span
        className={online ? "avatar__status avatar__status--online" : "avatar__status"}
        aria-label={onlineLabel}
      />
    </span>
  );
}
