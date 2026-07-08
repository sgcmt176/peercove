// デーモン・設定操作の型と、frontend から呼ぶ薄いラッパ。
//
// Rust 側(src-tauri/src/dto.rs)が明示的にこの形へ変換する。serde の内部タグ
// 表現を UI が直接なぞらないことで、プロトコルの変更が UI に波及しない。

import { invoke } from "@tauri-apps/api/core";

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
  /** トンネル内 RTT。制御接続が確立するまでは null。 */
  rttMs: number | null;
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

export interface ConfigSlot {
  path: string;
  exists: boolean;
}

export interface ConfigPaths {
  host: ConfigSlot;
  member: ConfigSlot;
  dir: string;
}

export interface InitResult {
  configPath: string;
  subnet: string;
  hostIp: string;
  publicKey: string;
}

/** token は秘密情報。発行直後のダイアログでのみ表示する(ADR-0008)。 */
export interface InviteResult {
  token: string;
  qrSvg: string;
  name: string;
  ip: string;
  endpoints: string[];
  psk: boolean;
}

export interface JoinResult {
  configPath: string;
  name: string;
  address: string;
  endpoint: string;
  otherEndpoints: string[];
}

/** 設定ファイルの現在値(M2-G5)。 */
export interface Settings {
  interfaceName: string;
  displayName: string | null;
  address: string;
  listenPort: number | null;
  mtu: number;
  hostEndpoint: string | null;
  isMember: boolean;
  defaultMtu: number;
  defaultListenPort: number;
}

export interface SettingsUpdate {
  displayName: string | null;
  listenPort: number | null;
  mtu: number;
  hostEndpoint: string | null;
}

export interface SaveResult {
  /** MTU / 待受ポート / エンドポイントを変えた場合。再接続まで反映されない。 */
  restartRequired: boolean;
}

export interface LogEntry {
  seq: number;
  unixMs: number;
  level: string;
  target: string;
  message: string;
}

export interface Logs {
  lines: LogEntry[];
  /** バッファから溢れて失われた行数。 */
  dropped: number;
}

/** UI が扱う接続状態。デーモン自体へ届かない場合を含む。 */
export type Connection =
  | { kind: "connecting" }
  | { kind: "unreachable"; message: string }
  | { kind: "ok"; status: Status };

// ---- コマンド ----

export const api = {
  daemonStatus: () => invoke<Status>("daemon_status"),
  startHost: (configPath: string, upnp: boolean) =>
    invoke<void>("start_host", { configPath, upnp }),
  startMember: (configPath: string) =>
    invoke<void>("start_member", { configPath }),
  stopTunnel: () => invoke<void>("stop_tunnel"),
  configPaths: () => invoke<ConfigPaths>("config_paths"),
  initHost: (force: boolean) => invoke<InitResult>("init_host", { force }),
  createInvite: (
    configPath: string,
    name: string | null,
    psk: boolean,
    endpoints: string[],
  ) => invoke<InviteResult>("create_invite", { configPath, name, psk, endpoints }),
  joinNetwork: (token: string, force: boolean) =>
    invoke<JoinResult>("join_network", { token, force }),
  removeMember: (configPath: string, publicKey: string) =>
    invoke<string>("remove_member", { configPath, publicKey }),
  renameMember: (configPath: string, publicKey: string, newName: string) =>
    invoke<void>("rename_member", { configPath, publicKey, newName }),
  daemonLogs: (afterSeq: number) => invoke<Logs>("daemon_logs", { afterSeq }),
  readSettings: (configPath: string) =>
    invoke<Settings>("read_settings", { configPath }),
  saveSettings: (configPath: string, update: SettingsUpdate) =>
    invoke<SaveResult>("save_settings", { configPath, update }),
};

// ---- 表示ヘルパ ----

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

/** 1 ミリ秒未満は「< 1 ms」。ローカルの検証で 0.0 ms と出るのを避ける。 */
export function formatRtt(rttMs: number | null): string {
  if (rttMs === null) return "—";
  if (rttMs < 1) return "< 1 ms";
  return `${rttMs.toFixed(rttMs < 10 ? 1 : 0)} ms`;
}

/** ログの時刻はローカルタイムで表示する(デーモンは UNIX ミリ秒で返す)。 */
export function formatLogTime(unixMs: number): string {
  const at = new Date(unixMs);
  const pad = (value: number, width = 2) => String(value).padStart(width, "0");
  return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}.${pad(at.getMilliseconds(), 3)}`;
}

/** invoke のエラーは文字列で返る。 */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
