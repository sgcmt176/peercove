// デーモンの状態を表す UI 用の型。
//
// Rust 側(src-tauri/src/lib.rs)が `peercove-core::ipc::DaemonStatus` から
// この形へ明示的に変換する。serde の内部タグ表現を UI が直接なぞらないことで、
// プロトコルの表現変更が UI に波及しないようにしている。

export interface Member {
  name: string | null;
  ip: string;
  publicKey: string;
  online: boolean;
  isHost: boolean;
}

export interface Peer {
  publicKey: string;
  endpoint: string | null;
  lastHandshakeAgeSecs: number | null;
  rxBytes: number;
  txBytes: number;
}

export interface Tunnel {
  config: string;
  address: string;
  members: Member[];
  peers: Peer[];
}

/** 同時参加は 1 ネットワークまで(M2 handoff Q4)。 */
export type TunnelState = "idle" | "hosting" | "joined";

export interface Status {
  state: TunnelState;
  tunnel: Tunnel | null;
}

/** UI が扱う接続状態。デーモン自体へ届かない場合を含む。 */
export type Connection =
  | { kind: "connecting" }
  | { kind: "unreachable"; message: string }
  | { kind: "ok"; status: Status };

export function stateLabel(state: TunnelState): string {
  switch (state) {
    case "idle":
      return "待機中";
    case "hosting":
      return "ホストとして稼働中";
    case "joined":
      return "メンバーとして参加中";
  }
}

export function formatBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(2)} ${units[unit]}`;
}

export function formatHandshake(ageSecs: number | null): string {
  if (ageSecs === null) return "なし";
  if (ageSecs < 60) return `${ageSecs} 秒前`;
  const minutes = Math.floor(ageSecs / 60);
  if (minutes < 60) return `${minutes} 分前`;
  return `${Math.floor(minutes / 60)} 時間前`;
}
