// node_modules を走査して npm 依存のライセンス表示を集める。
// make-notices.sh から呼ばれる。引数: node_modules のパス。
//
// 各パッケージの package.json から name/version/license を読み、隣接する
// LICENSE 系ファイルがあれば本文も出力する。開発専用依存(devDependencies)も
// 配布 JS バンドルに混ざりうるため区別せず全て含める(過不足なく表示する側に倒す)。

import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { join } from "node:path";

const nmRoot = process.argv[2];
if (!nmRoot) {
  console.error("usage: collect-npm-licenses.mjs <node_modules>");
  process.exit(1);
}

const LICENSE_FILES = [
  "LICENSE", "LICENSE.md", "LICENSE.txt", "license", "license.md",
  "LICENCE", "LICENCE.md", "LICENCE.txt", "COPYING", "COPYING.md",
];

/** node_modules 配下のパッケージディレクトリを列挙(スコープ @foo/bar も辿る)。 */
function findPackages(dir) {
  const out = [];
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const e of entries) {
    if (!e.isDirectory() && !e.isSymbolicLink()) continue;
    if (e.name === ".bin" || e.name === ".cache") continue;
    const full = join(dir, e.name);
    if (e.name.startsWith("@")) {
      // スコープ配下の各パッケージ
      out.push(...findPackages(full));
      continue;
    }
    if (existsSync(join(full, "package.json"))) out.push(full);
    // ネストした node_modules も辿る
    const nested = join(full, "node_modules");
    if (existsSync(nested)) out.push(...findPackages(nested));
  }
  return out;
}

function readLicenseText(pkgDir) {
  for (const f of LICENSE_FILES) {
    const p = join(pkgDir, f);
    try {
      if (statSync(p).isFile()) return readFileSync(p, "utf8").trim();
    } catch {
      /* 無ければ次 */
    }
  }
  return null;
}

function licenseField(pkg) {
  if (typeof pkg.license === "string") return pkg.license;
  if (pkg.license && typeof pkg.license === "object" && pkg.license.type) return pkg.license.type;
  if (Array.isArray(pkg.licenses)) return pkg.licenses.map((l) => l.type || l).join(" OR ");
  return "(未記載)";
}

const seen = new Map(); // name@version -> {license, text}
for (const dir of findPackages(nmRoot)) {
  let pkg;
  try {
    pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  } catch {
    continue;
  }
  if (!pkg.name || !pkg.version) continue;
  const key = `${pkg.name}@${pkg.version}`;
  if (seen.has(key)) continue;
  seen.set(key, { license: licenseField(pkg), text: readLicenseText(dir) });
}

const keys = [...seen.keys()].sort((a, b) => a.localeCompare(b));
const sep = "=".repeat(80);
const dash = "-".repeat(80);

let out = "";
for (const key of keys) {
  const { license, text } = seen.get(key);
  out += `${sep}\n${key} — ${license}\n`;
  if (text) out += `${dash}\n${text}\n`;
  out += "\n";
}
process.stdout.write(out);
console.error(`npm パッケージ ${keys.length} 件を収集しました`);
