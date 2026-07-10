// ピア統計の時系列バッファ(M3-6 スパークライン用)。
//
// App のポーリング(2 秒間隔)のたびに Status 全体を記録し、ピアごとの
// 転送速度(バイト/秒)と RTT の直近の推移を保持する。表示コンポーネントが
// マウントされる前から溜めたいので、React の state ではなくモジュールの
// Map に置く(ネットワーク詳細を開いた瞬間からグラフが出る)。

import { Status } from "./ipc";

/** 保持するサンプル数。2 秒間隔 × 45 = 直近およそ 90 秒。 */
const MAX_SAMPLES = 45;

interface Sample {
  /** 記録時刻(ms)。速度はサンプル間の実測経過時間で割る。 */
  at: number;
  rxBytes: number;
  txBytes: number;
  rttMs: number | null;
}

/** `<configPath>|<publicKey>` → サンプル列(古い順)。 */
const samples = new Map<string, Sample[]>();

function key(config: string, publicKey: string): string {
  return `${config}|${publicKey}`;
}

/** ポーリング結果を記録する。止まったトンネルの履歴は捨てる。 */
export function recordStatus(status: Status): void {
  const seen = new Set<string>();
  const at = Date.now();
  for (const tunnel of status.tunnels) {
    for (const peer of tunnel.peers) {
      const k = key(tunnel.config, peer.publicKey);
      seen.add(k);
      const list = samples.get(k) ?? [];
      list.push({
        at,
        rxBytes: peer.rxBytes,
        txBytes: peer.txBytes,
        rttMs: peer.rttMs,
      });
      if (list.length > MAX_SAMPLES) list.shift();
      samples.set(k, list);
    }
  }
  for (const k of [...samples.keys()]) {
    if (!seen.has(k)) samples.delete(k);
  }
}

/** デーモンに届かなくなったら全履歴を捨てる(値が信用できないため)。 */
export function clearHistory(): void {
  samples.clear();
}

/**
 * 転送速度(受信+送信、バイト/秒)の推移。サンプル間の差分なので
 * 長さは記録数 - 1。カウンタが巻き戻った区間(トンネル再作成)は 0 扱い。
 */
export function rateSeries(config: string, publicKey: string): number[] {
  const list = samples.get(key(config, publicKey)) ?? [];
  const rates: number[] = [];
  for (let i = 1; i < list.length; i++) {
    const dt = (list[i].at - list[i - 1].at) / 1000;
    const delta =
      list[i].rxBytes - list[i - 1].rxBytes + list[i].txBytes - list[i - 1].txBytes;
    rates.push(dt > 0 && delta > 0 ? delta / dt : 0);
  }
  return rates;
}

/** RTT(ms)の推移。未計測の区間は null(スパークラインでは途切れて見える)。 */
export function rttSeries(config: string, publicKey: string): (number | null)[] {
  const list = samples.get(key(config, publicKey)) ?? [];
  return list.map((sample) => sample.rttMs);
}
