// 小さな折れ線グラフ(M3-6)。転送速度と RTT の直近の推移を数字の横に添える。
//
// 依存ライブラリなしの素の SVG。値のスケールは系列内の最大値に対する相対で、
// 軸もラベルも描かない(傾向を見るためのもの。正確な値は隣の数字が持つ)。

/** 描画領域。行の高さに収まる控えめなサイズ。 */
const WIDTH = 64;
const HEIGHT = 16;
const PAD = 1.5;

export function Sparkline({
  values,
  title,
}: {
  /** 古い順の系列。null は未計測(線が途切れる)。 */
  values: (number | null)[];
  title?: string;
}) {
  const numbers = values.filter((v): v is number => v !== null);
  // 2 点未満は線にならないので場所だけ確保する(レイアウトが跳ねないように)
  if (numbers.length < 2) {
    return <svg className="sparkline" width={WIDTH} height={HEIGHT} aria-hidden />;
  }

  const max = Math.max(...numbers, 1e-9);
  const step = (WIDTH - PAD * 2) / Math.max(values.length - 1, 1);
  const y = (v: number) => HEIGHT - PAD - (v / max) * (HEIGHT - PAD * 2);

  // null を挟んだら polyline を分割する(未計測区間を線で繋がない)
  const segments: string[] = [];
  let current: string[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (current.length > 1) segments.push(current.join(" "));
      current = [];
    } else {
      current.push(`${(PAD + i * step).toFixed(1)},${y(v).toFixed(1)}`);
    }
  });
  if (current.length > 1) segments.push(current.join(" "));

  return (
    <svg className="sparkline" width={WIDTH} height={HEIGHT} role="img">
      {title && <title>{title}</title>}
      {segments.map((points, i) => (
        <polyline key={i} points={points} />
      ))}
    </svg>
  );
}
