// デーモン・設定操作の型と、frontend から呼ぶ薄いラッパ。
//
// Rust 側(src-tauri/src/dto.rs)が明示的にこの形へ変換する。serde の内部タグ
// 表現を UI が直接なぞらないことで、プロトコルの変更が UI に波及しない。

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

export interface Member {
  name: string | null;
  ip: string;
  publicKey: string;
  online: boolean;
  isHost: boolean;
  /** DNS 名（M3-1。例: alice.game.peercove.internal）。 */
  dnsName: string | null;
  /**
   * このメンバーへの経路（M3-4）。メンバーとして参加中の他メンバーにのみ付く。
   * direct = 直接通信中 / trying = 確立中 / relay = ホスト経由。
   */
  route: "direct" | "trying" | "relay" | null;
  /** この行が自分自身か。 */
  isSelf: boolean;
  /** このメンバーが広告する背後 LAN のサブネット（M3-7、ADR-0014）。 */
  subnets: string[];
}

/** カスタム DNS レコード（M3-1c）。 */
export interface DnsRecord {
  name: string;
  ip: string;
  fqdn: string;
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

/** ファイル転送の進捗 1 件（ADR-0015、M3-9b）。 */
export interface Transfer {
  id: string;
  /** 自分から見た向き。 */
  direction: "send" | "recv";
  /** 相手の仮想 IP。 */
  peer: string;
  name: string;
  size: number;
  transferred: number;
  done: boolean;
  error: string | null;
}

/** 受信ボックスの 1 ファイル（ADR-0015、M3-9b）。 */
export interface InboxItem {
  name: string;
  size: number;
  fromName: string | null;
  fromIp: string | null;
  receivedUnixMs: number | null;
}

/** チャット履歴の 1 通（ADR-0016、M3-13b）。 */
export interface ChatMessage {
  /** 履歴内の通し番号（差分フェッチ・未読管理に使う）。 */
  seq: number;
  id: string;
  scope: "direct" | "network";
  /** 送信者の仮想 IP（自分が送った通は自分の IP）。 */
  from: string;
  /** (direct のみ)宛先の仮想 IP。 */
  to: string | null;
  text: string;
  sentAtMs: number;
  /** どの宛先にも届かなかった（デーモン再起動で消える）。 */
  failed: boolean;
}

/** チャット履歴の 1 ページ。messages の末尾が seq に届くまで繰り返し取る。 */
export interface ChatPage {
  seq: number;
  messages: ChatMessage[];
}

export interface Tunnel {
  config: string;
  /** ネットワーク名（ADR-0012）。 */
  network: string;
  /** このトンネルでの役割。 */
  role: "hosting" | "joined";
  address: string;
  members: Member[];
  peers: Peer[];
  /** ホストからネットワーク削除された（M2-G6）。UI が明示して切断を促す。 */
  removed: boolean;
  /** ファイル転送の進捗（実行中 + 直近の完了/失敗分）。 */
  transfers: Transfer[];
  /** チャット履歴の最新 seq（ADR-0016）。進んだら差分フェッチする。 */
  chatSeq: number;
}

/** 同時参加は 1 ネットワークまで(M2 handoff Q4)。 */
export type TunnelState = "idle" | "hosting" | "joined";

export interface Status {
  /** 複数稼働時は先頭トンネルの状態（互換用。一覧 UI は M3-0c）。 */
  state: TunnelState;
  /** 互換用: 先頭のトンネル。 */
  tunnel: Tunnel | null;
  /** 稼働中の全トンネル（ADR-0012）。 */
  tunnels: Tunnel[];
  /** デーモンが古い（IPC バージョン不一致）。状態表示は信用できない。 */
  daemonOutdated: boolean;
}

/** 設定済みネットワーク 1 件（M3-0c）。稼働状態は Status.tunnels と configPath で突き合わせる。 */
export interface NetworkInfo {
  slug: string;
  name: string;
  /** 設定上の役割。 */
  role: "hosting" | "joined";
  configPath: string;
  address: string;
}

export interface InitResult {
  configPath: string;
  network: string;
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
  /** メンバー間直接通信を試すか（ADR-0013、既定 true）。 */
  direct: boolean;
  /** 受信するファイルサイズの上限（MB、ADR-0015）。0 で無制限。 */
  maxRecvFileMb: number;
  defaultMtu: number;
  defaultListenPort: number;
  defaultMaxRecvFileMb: number;
}

export interface SettingsUpdate {
  displayName: string | null;
  listenPort: number | null;
  mtu: number;
  hostEndpoint: string | null;
  direct: boolean;
  maxRecvFileMb: number;
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
  stopTunnel: (configPath: string) =>
    invoke<void>("stop_tunnel", { configPath }),
  listNetworks: () => invoke<NetworkInfo[]>("list_networks"),
  deleteNetwork: (slug: string) => invoke<void>("delete_network", { slug }),
  initHost: (name: string, force: boolean) =>
    invoke<InitResult>("init_host", { name, force }),
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
  setMemberSubnets: (configPath: string, publicKey: string, subnets: string[]) =>
    invoke<void>("set_member_subnets", { configPath, publicKey, subnets }),
  // チャット（ADR-0016、M3-13b）。peer = null でネットワーク全体宛
  chatSend: (configPath: string, peer: string | null, text: string) =>
    invoke<ChatMessage>("chat_send", { configPath, peer, text }),
  chatFetch: (configPath: string, afterSeq: number) =>
    invoke<ChatPage>("chat_fetch", { configPath, afterSeq }),
  // ファイル送信・受信ボックス（ADR-0015、M3-9b）
  pickFile: () => invoke<string | null>("pick_file"),
  sendFile: (configPath: string, peer: string, path: string) =>
    invoke<string>("send_file", { configPath, peer, path }),
  listInbox: (configPath: string) =>
    invoke<InboxItem[]>("list_inbox", { configPath }),
  saveInboxFile: (configPath: string, name: string) =>
    invoke<string | null>("save_inbox_file", { configPath, name }),
  deleteInboxFile: (configPath: string, name: string) =>
    invoke<void>("delete_inbox_file", { configPath, name }),
  daemonLogs: (afterSeq: number) => invoke<Logs>("daemon_logs", { afterSeq }),
  listDnsRecords: (configPath: string) =>
    invoke<DnsRecord[]>("list_dns_records", { configPath }),
  addDnsRecord: (configPath: string, name: string, ip: string) =>
    invoke<void>("add_dns_record", { configPath, name, ip }),
  removeDnsRecord: (configPath: string, name: string) =>
    invoke<void>("remove_dns_record", { configPath, name }),
  readSettings: (configPath: string) =>
    invoke<Settings>("read_settings", { configPath }),
  saveSettings: (configPath: string, update: SettingsUpdate) =>
    invoke<SaveResult>("save_settings", { configPath, update }),
};

// ---- 表示ヘルパ ----

export function stateLabel(state: TunnelState): string {
  switch (state) {
    case "idle":
      return t.state.idle;
    case "hosting":
      return t.state.hosting;
    case "joined":
      return t.state.joined;
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
  if (ageSecs === null) return t.format.none;
  if (ageSecs < 60) return t.format.secondsAgo(ageSecs);
  const minutes = Math.floor(ageSecs / 60);
  if (minutes < 60) return t.format.minutesAgo(minutes);
  return t.format.hoursAgo(Math.floor(minutes / 60));
}

/** 転送速度(バイト/秒)。まだ差分が取れないうちは「—」(M3-6)。 */
export function formatRate(bytesPerSec: number | null): string {
  if (bytesPerSec === null) return "—";
  return `${formatBytes(Math.round(bytesPerSec))}/s`;
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
