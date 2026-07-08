// アプリアイコンの元画像(1024x1024 PNG)を生成する。
// 外部素材に依存しないよう、Node の zlib だけで PNG を組み立てる。
// 使い方: node scripts/make-icon.mjs  → src-tauri/icons/source.png
// その後 `npx tauri icon src-tauri/icons/source.png` で各サイズを生成する。

import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const SIZE = 1024;
const OUT = "src-tauri/icons/source.png";

// PeerCove: 入り江(cove)に集まるピアのイメージ。
// 濃紺の角丸背景に、中心のホストと周囲のメンバーを結ぶ星形。
const BG = [26, 42, 68]; // 濃紺
const HUB = [110, 168, 254]; // 明るい青
const SPOKE = [86, 214, 168]; // 緑青
const LINE = [58, 84, 124];

function blend(dst, src, alpha) {
  for (let i = 0; i < 3; i += 1) {
    dst[i] = Math.round(dst[i] * (1 - alpha) + src[i] * alpha);
  }
}

/** アンチエイリアス付きの円(中心 cx,cy 半径 r)。 */
function circleAlpha(x, y, cx, cy, r) {
  const d = Math.hypot(x - cx, y - cy);
  return Math.min(1, Math.max(0, r + 0.5 - d));
}

/** 線分 (x1,y1)-(x2,y2) からの距離に基づくアンチエイリアス。 */
function lineAlpha(x, y, x1, y1, x2, y2, halfWidth) {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len2 = dx * dx + dy * dy;
  let t = len2 === 0 ? 0 : ((x - x1) * dx + (y - y1) * dy) / len2;
  t = Math.min(1, Math.max(0, t));
  const d = Math.hypot(x - (x1 + t * dx), y - (y1 + t * dy));
  return Math.min(1, Math.max(0, halfWidth + 0.5 - d));
}

/** 角丸矩形の内側かどうか(アルファ)。 */
function roundedRectAlpha(x, y, size, radius) {
  const inset = 0;
  const min = inset;
  const max = size - 1 - inset;
  const cx = Math.min(Math.max(x, min + radius), max - radius);
  const cy = Math.min(Math.max(y, min + radius), max - radius);
  const d = Math.hypot(x - cx, y - cy);
  return Math.min(1, Math.max(0, radius + 0.5 - d));
}

const center = (SIZE - 1) / 2;
const spokes = [];
const spokeCount = 5;
const orbit = SIZE * 0.3;
for (let i = 0; i < spokeCount; i += 1) {
  const angle = (Math.PI * 2 * i) / spokeCount - Math.PI / 2;
  spokes.push([center + orbit * Math.cos(angle), center + orbit * Math.sin(angle)]);
}

// RGBA ピクセルを作る
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1)); // 各行の先頭にフィルタバイト
let p = 0;
for (let y = 0; y < SIZE; y += 1) {
  raw[p] = 0; // filter: None
  p += 1;
  for (let x = 0; x < SIZE; x += 1) {
    const bgA = roundedRectAlpha(x, y, SIZE, SIZE * 0.22);
    const px = [...BG];

    for (const [sx, sy] of spokes) {
      const a = lineAlpha(x, y, center, center, sx, sy, SIZE * 0.012);
      if (a > 0) blend(px, LINE, a);
    }
    for (const [sx, sy] of spokes) {
      const a = circleAlpha(x, y, sx, sy, SIZE * 0.055);
      if (a > 0) blend(px, SPOKE, a);
    }
    const hub = circleAlpha(x, y, center, center, SIZE * 0.1);
    if (hub > 0) blend(px, HUB, hub);

    raw[p] = px[0];
    raw[p + 1] = px[1];
    raw[p + 2] = px[2];
    raw[p + 3] = Math.round(bgA * 255);
    p += 4;
  }
}

function crc32(buf) {
  let c = ~0;
  for (const byte of buf) {
    c ^= byte;
    for (let k = 0; k < 8; k += 1) {
      c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
    }
  }
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type: RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, png);
console.log(`${OUT} を生成しました (${SIZE}x${SIZE}, ${png.length} bytes)`);
