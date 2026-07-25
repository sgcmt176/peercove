// 名前の自然順ソート(M6 H-9c)。「member-9」が「member-11」より前に来るよう、
// 数字部分は文字列比較でなく数値比較する。
//
// 分解ルール: 末尾の連続する数字だけを「数字部分」として切り出し、残りを
// 「接頭辞」とする(例: "member-9" → 接頭辞 "member-", 数字 9)。比較は
// 接頭辞の辞書順 → 数字の数値順の順。末尾に数字が無い名前(例: "スマホ")は
// 数字部分を持たないものとして扱い、常に同じ接頭辞内の数字付きより後ろに
// 並ぶ(数字なし = 無限大相当)。これにより「同じ接頭辞なら数字が若い順、
// 数字が無いものは最後」という一貫した規則になる。

interface NameParts {
  prefix: string;
  /** 末尾の数字部分。無ければ null(常に数字ありより後ろに並べる)。 */
  suffixNumber: number | null;
}

function splitTrailingNumber(name: string): NameParts {
  const match = name.match(/^(.*?)(\d+)$/);
  if (!match) return { prefix: name, suffixNumber: null };
  return { prefix: match[1], suffixNumber: Number(match[2]) };
}

/** 自然順比較。Array.prototype.sort にそのまま渡せる。 */
export function compareNatural(a: string, b: string): number {
  const pa = splitTrailingNumber(a);
  const pb = splitTrailingNumber(b);
  const prefixCmp = pa.prefix.localeCompare(pb.prefix);
  if (prefixCmp !== 0) return prefixCmp;
  if (pa.suffixNumber === null && pb.suffixNumber === null) return 0;
  if (pa.suffixNumber === null) return 1;
  if (pb.suffixNumber === null) return -1;
  return pa.suffixNumber - pb.suffixNumber;
}
