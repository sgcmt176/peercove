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
  /** 相手が Hello で広告した製品バージョン。旧版は null。 */
  appVersion: string | null;
  /** 相手の OS（"windows" / "linux" / "android"）。旧版・オフライン中は null。 */
  platform: string | null;
  /** 相手が広告した追加機能 ID。旧版は空。 */
  capabilities: string[];
  inviteStatus:
    | "legacy"
    | "pending"
    | "joined"
    | "awaiting_approval"
    | "expired"
    | "clock_invalid"
    | null;
  inviteExpiresAt: number | null;
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
  /** member_id（= invite_id、ADR-0047）。共有メモの個別権限指定に使う。 */
  memberId: string | null;
  /** このメンバーが広告する背後 LAN のサブネット（M3-7、ADR-0014）。 */
  subnets: string[];
  /** 自分とこのメンバーの間がホストの ACL で遮断されている（M3-10、ADR-0018）。 */
  blocked: boolean;
  /** ACL v2のためホスト中継へ固定されている。 */
  forceRelay: boolean;
  aclRuleId: string | null;
  /** この端末がメンバー招待を発行できるか（ADR-0048）。自分の行の値でボタン表示を決める。 */
  canInvite: boolean;
  /** このメンバーを招待した発行者の表示名（ADR-0048）。ホスト発行は null。 */
  invitedBy: string | null;
}

/** カスタム DNS レコード（M3-1c、ADR-0022 で拡張）。 */
export interface DnsRecord {
  id: string | null;
  name: string;
  /** 解決済みの現在の IP（メンバー参照が切れている場合のみ null）。 */
  ip: string | null;
  fqdn: string;
  /** ターゲットのメンバー参照（"host" または公開鍵）。固定 IP レコードは null。 */
  member: string | null;
  /** CNAME の転送先ドメイン（A / メンバー参照レコードは null — ADR-0025）。 */
  cname: string | null;
  /** 親メンバー（"host" または公開鍵）。最上位レコードは null。 */
  under: string | null;
  /** URL コピー用のサービス情報（M3-14c、ADR-0023）。 */
  scheme: string | null;
  port: number | null;
  /** スキームがある場合にバックエンドで組み立て済みの URL。 */
  url: string | null;
  health: ServiceHealth | null;
  healthSettings: HealthSettings | null;
}

export interface ServiceHealth {
  status: "healthy" | "unhealthy" | "unknown" | "disabled";
  reason:
    | "not_checked"
    | "offline"
    | "timeout"
    | "connection_failed"
    | "name_resolution_failed"
    | "unexpected_status"
    | "disabled";
  checkedAtUnixMs: number | null;
  responseMs: number | null;
  httpStatus: number | null;
}

export interface HealthSettings {
  enabled: boolean;
  kind: "tcp" | "http_head";
  path: string;
  expectedStatus: number | null;
  external: boolean;
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
  scope: "direct" | "network" | "group";
  /** (group のみ)宛先グループの ID（M3-13c）。 */
  groupId: string | null;
  /** 送信者の仮想 IP（自分が送った通は自分の IP）。 */
  from: string;
  /** (direct のみ)宛先の仮想 IP。 */
  to: string | null;
  text: string;
  sentAtMs: number;
  /** どの宛先にも届かなかった（デーモン再起動で消える）。 */
  failed: boolean;
  /** チャット内ファイル送信のエントリ（M3-13d）。付いていれば text は空。 */
  file: ChatFile | null;
  /** グループ操作のお知らせ（中央の 1 行として表示。未読・通知の対象外）。 */
  system: boolean;
}

/** チャット内ファイル送信の情報（M3-13d）。実体は受信ボックス。 */
export interface ChatFile {
  /** ファイル名（受信側では保存された実ファイル名）。 */
  name: string;
  size: number;
  /** 対応する転送 id（Tunnel.transfers と突き合わせて進捗を出す）。 */
  transfers: string[];
  /** この端末でのファイルの場所（インラインプレビュー用）。 */
  path: string | null;
}

/** テキストファイルのプレビュー（M3-13e）。先頭だけ読んだもの。 */
export interface TextPreview {
  text: string;
  /** 上限（256 KiB）で打ち切った。 */
  truncated: boolean;
}

/** リンクプレビュー（M3-13e、ADR-0017）。image は data URI。 */
export interface LinkPreview {
  title: string | null;
  description: string | null;
  siteName: string | null;
  image: string | null;
}

/** ファイル送信のチャット文脈（M3-13d）。 */
export interface ChatContext {
  scope: "direct" | "network" | "group";
  groupId?: string | null;
}

/** グループ（ADR-0016、M3-13c）。members に自分が居なければ「退出済み」。 */
export interface Group {
  id: string;
  name: string;
  /** メンバーの仮想 IP。 */
  members: string[];
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
  /** 実行時のインターフェース名（自動採番後。設定値と異なりうる — M3-20）。 */
  interfaceName: string;
  members: Member[];
  peers: Peer[];
  /** ホストからネットワーク削除された（M2-G6）。UI が明示して切断を促す。 */
  removed: boolean;
  /** ホストが参加を拒否し、自動再接続を停止した理由。 */
  connectionError?: string | null;
  /** ファイル転送の進捗（実行中 + 直近の完了/失敗分）。 */
  transfers: Transfer[];
  /** チャット履歴の最新 seq（ADR-0016）。進んだら差分フェッチする。 */
  chatSeq: number;
  /** チャット履歴の消去世代。変わったら手元の履歴を捨てて取り直す。 */
  chatGeneration: number;
  /** 送信待ち（再送キューに残っている）チャットの seq（E-E 3）。 */
  chatSending: number[];
  /** 既知のグループ（M3-13c）。自分が抜けたグループも含む。 */
  groups: Group[];
  /** 共有メモの変更世代（M5 F-2）。進んだら再取得する。 */
  sharedMemoSeq: number;
  /** 共有メモが使えるか（member で false = ホスト未対応 or 未同期）。 */
  sharedMemo: boolean;
  /** 共有メモの権限ダイアログで選べるグループ（ADR-0051）。host は既知の全グループ、member は自分の所属グループだけ。 */
  permGroups?: PermGroup[];
  /** 解決済みカスタム DNS レコード（ADR-0022）。member はここから一覧表示する。 */
  dnsRecords: DnsRecord[];
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
  /** デーモン実行ファイルの製品バージョン。旧デーモンは null。 */
  daemonVersion: string | null;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  available: boolean;
  releaseUrl: string;
  releaseName: string | null;
  publishedAt: string | null;
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

export interface BackupPreview {
  networkName: string;
  role: "host" | "member";
  sourceOs: string;
  createdAtUnixMs: number;
  categories: string[];
  configFile: string;
  memberKeyRotationRecommended: boolean;
}

export type AclAction = "allow" | "deny";
export type AclProtocol = "any" | "tcp" | "udp" | "icmp";
export type AclTarget = "any" | { member: string } | { group: string } | { subnet: string } | { service: string };
export interface AclGroup { id: string; members: string[]; }
export interface AclRule { id: string; action: AclAction; source: AclTarget; destination: AclTarget; protocol: AclProtocol; ports: string[]; enabled: boolean; }
export interface AclPolicySettings { default: AclAction; groups: AclGroup[]; rules: AclRule[]; }

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
  inviteId: string;
  issuedAt: number;
  expiresAt: number | null;
}

/** メンバー発行の招待（ADR-0048）。token は秘密情報（発行直後のみ表示）。 */
export interface MemberInviteResult {
  token: string;
  qrSvg: string;
  name: string;
  expiresAt: number | null;
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
  /** (host のみ)自分の DNS 名（ADR-0021、M3-14a）。未設定なら実質 host。 */
  dnsName: string | null;
  address: string;
  listenPort: number | null;
  mtu: number;
  hostEndpoint: string | null;
  isMember: boolean;
  /** メンバー間直接通信を試すか（ADR-0013、既定 true）。 */
  direct: boolean;
  /** 受信するファイルサイズの上限（MB、ADR-0015）。0 で無制限。 */
  maxRecvFileMb: number;
  /** ホストのみ。新規参加端末を承認まで隔離する。 */
  requireInviteApproval: boolean;
  /** ホストのみ。メンバーによる招待発行を許可する（ADR-0048、既定 true）。 */
  memberInvites: boolean;
  defaultMtu: number;
  defaultListenPort: number;
  defaultMaxRecvFileMb: number;
}

export interface SettingsUpdate {
  displayName: string | null;
  /** (host のみ)自分の DNS 名（ADR-0021）。null / 空で既定（host）に戻す。 */
  dnsName: string | null;
  listenPort: number | null;
  mtu: number;
  hostEndpoint: string | null;
  direct: boolean;
  maxRecvFileMb: number;
  requireInviteApproval: boolean;
  /** ホストのみ。メンバーによる招待発行を許可する（ADR-0048）。 */
  memberInvites: boolean;
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

export type DiagnosticStatus = "pass" | "warning" | "fail" | "unknown";
export type DiagnosticOverall = "healthy" | "attention" | "problem";
export type DiagnosticCategory =
  | "app"
  | "tunnel"
  | "internet"
  | "dns"
  | "permissions"
  | "memo";

export interface DiagnosticCheck {
  id: string;
  category: DiagnosticCategory;
  status: DiagnosticStatus;
  evidence: Record<string, string>;
}

export interface DiagnosticReport {
  generated_at_unix_ms: number;
  scope: {
    config: string;
    network?: string;
    role?: string;
  };
  overall: DiagnosticOverall;
  checks: DiagnosticCheck[];
  logs: Array<{
    seq: number;
    unix_ms: number;
    level: string;
    target: string;
    message: string;
  }>;
}

export interface QualityPoint {
  windowStartUnixMs: number;
  windowSecs: number;
  publicKey: string;
  ip: string;
  name: string | null;
  availability: "connected" | "disconnected" | "unmeasured";
  rttLatestMs: number | null;
  rttMinMs: number | null;
  rttAvgMs: number | null;
  rttP95Ms: number | null;
  jitterMs: number | null;
  probesSent: number;
  probesReceived: number;
  lossPercent: number | null;
  route: "direct" | "relay" | "trying";
  routeSwitches: number;
  rxBytes: number;
  txBytes: number;
}

export interface QualityReport {
  generatedAtUnixMs: number;
  retentionDays: number;
  skippedCorruptLines: number;
  samples: QualityPoint[];
}

// ---- 個人メモ (M5 F-1, ADR-0049) ----
// 型は Rust 側(peercove-core::memo)の serde 表現をそのまま使うため、この
// セクションだけ snake_case。時刻は UNIX ミリ秒

export type MemoScope = "active" | "archived" | "trash";
export type MemoSort = "updated" | "created" | "title";

export interface MemoQuery {
  scope: MemoScope;
  folder_id?: string;
  tag?: string;
  search?: string;
  sort: MemoSort;
}

export interface MemoFolder {
  id: string;
  name: string;
  memo_count: number;
}

export interface MemoTagCount {
  tag: string;
  count: number;
}

export interface MemoSummary {
  id: string;
  title: string;
  excerpt: string;
  folder_id?: string;
  tags?: string[];
  pinned?: boolean;
  archived?: boolean;
  created_at: number;
  updated_at: number;
  deleted_at?: number;
  checklist_done?: number;
  checklist_total?: number;
}

export interface MemoDetail {
  id: string;
  title: string;
  body: string;
  folder_id?: string;
  tags?: string[];
  pinned?: boolean;
  archived?: boolean;
  created_at: number;
  updated_at: number;
  deleted_at?: number;
}

/** 部分更新。省略 = 変更しない。folder は `{}` で「フォルダーなし」へ移動。 */
export interface MemoPatch {
  title?: string;
  body?: string;
  folder?: { id?: string };
  pinned?: boolean;
  archived?: boolean;
  tags?: string[];
}

export type MemoOp =
  | { op: "list"; query: MemoQuery }
  | { op: "get"; id: string }
  // メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)。
  | { op: "resolve_titles"; titles: string[] }
  | { op: "backlinks"; id: string }
  | {
      op: "create";
      title: string;
      body: string;
      folder_id?: string;
      tags?: string[];
    }
  | { op: "update"; id: string; patch: MemoPatch }
  | { op: "duplicate"; id: string }
  | { op: "trash"; id: string }
  | { op: "restore"; id: string }
  | { op: "delete_forever"; id: string }
  | { op: "empty_trash" }
  | { op: "folder_create"; name: string }
  | { op: "folder_rename"; id: string; name: string }
  | { op: "folder_delete"; id: string }
  // リマインダー(端末ローカル、ADR-0052 決定 6)。共有メモに対する
  // 「自分用リマインダー」もここへ(network = 共有メモの configPath)。
  | {
      op: "reminder_set";
      scope: ReminderScope;
      network?: string;
      memo_id: string;
      remind_at: number;
    }
  | {
      op: "reminder_clear";
      scope: ReminderScope;
      network?: string;
      memo_id: string;
    }
  | { op: "reminder_list" }
  | { op: "reminder_take_due" };

export type ReminderScope = "personal" | "shared";

export interface MemoReminder {
  scope: ReminderScope;
  network?: string;
  memo_id: string;
  remind_at: number;
}

export type MemoReply =
  | {
      kind: "memos";
      memos: MemoSummary[];
      folders: MemoFolder[];
      tags: MemoTagCount[];
    }
  | { kind: "memo"; memo: MemoDetail }
  // ResolveTitles への応答(タイトル → memo_id、見つかったものだけ)。
  | { kind: "titles"; map: Record<string, string> }
  | { kind: "folder"; folder: MemoFolder }
  // ReminderList / ReminderTakeDue への応答。
  | { kind: "reminders"; reminders: MemoReminder[] }
  | { kind: "done" };

// ---- 共有メモ (M5 F-2, ADR-0049)。個人メモと同じく serde 表現(snake_case) ----

export type SharedPermLevel = "none" | "viewer" | "editor";

export interface SharedMemberPerm {
  member_id: string;
  name?: string;
  level: SharedPermLevel;
}

/** グループ単位の権限指定(ADR-0051)。 */
export interface SharedGroupPerm {
  group_id: string;
  name?: string;
  level: SharedPermLevel;
}

/** 共有メモの権限ダイアログで選べるグループ(ADR-0051)。id + 現在名だけ。 */
export interface PermGroup {
  id: string;
  name: string;
}

export interface SharedMemoQuery {
  trash?: boolean;
  folder_id?: string;
  search?: string;
}

export interface SharedMemoSummary {
  id: string;
  title: string;
  excerpt: string;
  folder_id?: string;
  revision: number;
  created_at: number;
  updated_at: number;
  updated_by?: string;
  owner_id: string;
  owner_name: string;
  deleted_at?: number;
  can_edit?: boolean;
  can_manage?: boolean;
  locked_by?: string;
  checklist_done?: number;
  checklist_total?: number;
  /** コメント件数(ADR-0052 決定 4)。一覧の 💬 バッジ用。 */
  comment_count?: number;
}

export interface SharedMemoDetail {
  id: string;
  title: string;
  body: string;
  folder_id?: string;
  revision: number;
  created_at: number;
  updated_at: number;
  updated_by?: string;
  owner_id: string;
  owner_name: string;
  deleted_at?: number;
  can_edit?: boolean;
  can_manage?: boolean;
  locked_by?: string;
  everyone?: SharedPermLevel;
  members?: SharedMemberPerm[];
  groups?: SharedGroupPerm[];
  /** コメント件数(ADR-0052 決定 4)。 */
  comment_count?: number;
}

/** 共有メモのコメント 1 件(ADR-0052 決定 4)。単層(返信ツリーなし)。 */
export interface SharedMemoComment {
  comment_id: string;
  memo_id: string;
  author_id: string;
  author_name: string;
  body: string;
  created_at_unix_ms: number;
}

/** 共有メモの容量・履歴の上限(ホスト設定可、M5 F-3)。 */
export interface SharedMemoLimits {
  max_body_bytes: number;
  max_memo_count: number;
  max_total_bytes: number;
  max_versions: number;
  history_days: number;
  trash_days: number;
}

/** 変更履歴 1 版分の要約(本文は含まない)。 */
export interface SharedMemoHistoryEntry {
  hid: number;
  revision: number;
  kind: "auto" | "close" | "manual" | "restore";
  saved_by_name: string;
  created_at_unix_ms: number;
  title: string;
  body_bytes: number;
}

/** 変更履歴 1 版分の全体(本文込み)。 */
export interface SharedMemoHistoryDetail {
  entry: SharedMemoHistoryEntry;
  body: string;
}

export type DiffLineKind = "same" | "added" | "removed";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

// ---- 共有スケジュール表(M6 G-1、ADR-0053)。SharedMemoOp/Reply に
// additive な `schedule` variant として相乗りする(crates/peercove-core/
// src/schedule.rs の serde 表現に一致させる)。 ----

export interface ScheduleEvent {
  id: string;
  title: string;
  note?: string;
  start_unix_ms: number;
  end_unix_ms?: number;
  /** 終日予定は日付単位で扱う(ADR-0053 決定 2)。 */
  all_day?: boolean;
  /** 所有者(作成者)の member_id。空文字 = ホスト。 */
  owner_id: string;
  owner_name: string;
  updated_by: string;
  revision: number;
  created_at: number;
  updated_at: number;
  /** 受信者視点: 編集・削除できるか(作成者 + ホスト)。 */
  can_edit?: boolean;
}

export type ScheduleOp =
  | { op: "list" }
  | {
      op: "create";
      title: string;
      note?: string;
      start_unix_ms: number;
      end_unix_ms?: number;
      all_day?: boolean;
    }
  | {
      op: "update";
      id: string;
      base_revision: number;
      title: string;
      note?: string;
      start_unix_ms: number;
      end_unix_ms?: number;
      all_day?: boolean;
    }
  | { op: "delete"; id: string };

export type ScheduleReply =
  | { kind: "events"; events: ScheduleEvent[]; offline?: boolean }
  | { kind: "event"; event: ScheduleEvent }
  | { kind: "done" }
  | { kind: "err"; message: string };

// ---- 共有シート(Excel ライク表、M6 G-2、ADR-0054)。SharedMemoOp/Reply に
// additive な `sheet` variant として相乗りする(crates/peercove-core/
// src/sheet.rs の serde 表現に一致させる)。 ----

export interface SheetMeta {
  id: string;
  name: string;
  /** 所有者(作成者)の member_id。空文字 = ホスト。 */
  owner_id: string;
  owner_name: string;
  created_at: number;
  updated_at: number;
  /** 受信者視点: 改名・削除できるか(作成者 + ホスト)。 */
  can_manage?: boolean;
}

export interface SheetCell {
  row: number;
  col: number;
  value: string;
  /** 単調増加リビジョン(CAS 用)。 */
  revision: number;
  updated_by: string;
  updated_at: number;
}

/** セル書き込み 1 件分。base_revision 0 = 新規セル想定。値が空文字ならセル削除。 */
export interface CellWrite {
  row: number;
  col: number;
  value: string;
  base_revision?: number;
}

export type SheetOp =
  | { op: "list" }
  | { op: "cells"; sheet_id: string }
  | { op: "create"; name?: string }
  | { op: "rename"; sheet_id: string; name?: string }
  | { op: "delete"; sheet_id: string }
  | { op: "write"; sheet_id: string; cells: CellWrite[] };

export type SheetReply =
  | { kind: "sheets"; sheets: SheetMeta[]; offline?: boolean }
  | { kind: "cells_data"; sheet_id: string; cells: SheetCell[]; offline?: boolean }
  | { kind: "sheet"; sheet: SheetMeta }
  | { kind: "write_result"; applied: number; conflicts: SheetCell[] }
  | { kind: "done" }
  | { kind: "err"; message: string };

export type SharedMemoOp =
  | { op: "list"; query: SharedMemoQuery }
  | { op: "get"; id: string }
  // メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)。
  | { op: "resolve_titles"; titles: string[] }
  | { op: "backlinks"; id: string }
  | { op: "create"; title: string; body: string; folder_id?: string }
  | {
      op: "update";
      id: string;
      base_revision: number;
      title: string;
      body: string;
    }
  | { op: "acquire_lock"; id: string }
  | { op: "release_lock"; id: string }
  | { op: "force_unlock"; id: string }
  | { op: "trash"; id: string }
  | { op: "restore"; id: string }
  | { op: "delete_forever"; id: string }
  | {
      op: "set_perms";
      id: string;
      everyone: SharedPermLevel;
      members?: SharedMemberPerm[];
      groups?: SharedGroupPerm[] | null;
    }
  | { op: "folder_create"; name: string }
  | { op: "folder_rename"; id: string; name: string }
  | { op: "folder_delete"; id: string }
  | { op: "history_list"; id: string }
  | { op: "history_get"; id: string; hid: number }
  | {
      op: "history_diff";
      id: string;
      from_hid: number;
      to_hid?: number;
    }
  | { op: "history_restore"; id: string; hid: number }
  | { op: "save_version"; id: string }
  | { op: "get_limits" }
  | { op: "set_limits"; limits: SharedMemoLimits }
  // コメント(ADR-0052 決定 4)。閲覧・追加 = viewer 以上、削除 = 本人・
  // 所有者・ホスト。メンバー経路は常時オンライン専用。
  | { op: "comment_list"; id: string }
  | { op: "comment_add"; id: string; body: string }
  | { op: "comment_delete"; id: string; comment_id: string }
  // 共有スケジュール表(M6 G-1、ADR-0053)。additive な相乗り。
  | { op: "schedule"; schedule: ScheduleOp }
  // 共有シート(M6 G-2、ADR-0054)。additive な相乗り。
  | { op: "sheet"; sheet: SheetOp };

export type SharedMemoReply =
  | {
      kind: "memos";
      memos: SharedMemoSummary[];
      folders: MemoFolder[];
      offline?: boolean;
    }
  | { kind: "memo"; memo: SharedMemoDetail }
  // ResolveTitles への応答(タイトル → memo_id、actor に見えるものだけ)。
  | { kind: "titles"; map: Record<string, string> }
  | { kind: "history"; entries: SharedMemoHistoryEntry[] }
  | { kind: "history_detail"; detail: SharedMemoHistoryDetail }
  | { kind: "diff"; lines: DiffLine[] }
  | { kind: "limits"; limits: SharedMemoLimits }
  // CommentList への応答(古い順、全件)。
  | { kind: "comments"; comments: SharedMemoComment[] }
  // CommentAdd への応答(追加した 1 件)。
  | { kind: "comment"; comment: SharedMemoComment }
  | { kind: "done" }
  | { kind: "err"; message: string }
  // 共有スケジュール表(M6 G-1、ADR-0053)。
  | { kind: "schedule"; reply: ScheduleReply }
  // 共有シート(M6 G-2、ADR-0054)。
  | { kind: "sheet"; reply: SheetReply };

/** UI が扱う接続状態。デーモン自体へ届かない場合を含む。 */
export type Connection =
  | { kind: "connecting" }
  | { kind: "unreachable"; message: string }
  | { kind: "ok"; status: Status };

// ---- コマンド ----

export const api = {
  // includeMemos は Option<bool> 側(未指定 = 同梱)。UI は常に明示して送る
  createBackup: (configPath: string, passphrase: string, includeMemos: boolean) =>
    invoke<string | null>("create_backup", { configPath, passphrase, includeMemos }),
  pickBackup: () => invoke<string | null>("pick_backup"),
  inspectBackup: (path: string, passphrase: string) =>
    invoke<BackupPreview>("inspect_backup", { path, passphrase }),
  restoreBackup: (path: string, passphrase: string, slug: string, replace: boolean) =>
    invoke<string>("restore_backup", { path, passphrase, slug, replace }),
  daemonStatus: () => invoke<Status>("daemon_status"),
  checkUpdate: () => invoke<UpdateInfo>("check_update"),
  startHost: (configPath: string, upnp: boolean) =>
    invoke<void>("start_host", { configPath, upnp }),
  startMember: (configPath: string) =>
    invoke<void>("start_member", { configPath }),
  stopTunnel: (configPath: string) =>
    invoke<void>("stop_tunnel", { configPath }),
  rotateKey: (configPath: string) => invoke<void>("rotate_key", { configPath }),
  listNetworks: () => invoke<NetworkInfo[]>("list_networks"),
  deleteNetwork: (slug: string) => invoke<void>("delete_network", { slug }),
  initHost: (name: string, force: boolean) =>
    invoke<InitResult>("init_host", { name, force }),
  createInvite: (
    configPath: string,
    name: string | null,
    psk: boolean,
    endpoints: string[],
    expiresInSecs: number | null,
  ) => invoke<InviteResult>("create_invite", { configPath, name, psk, endpoints, expiresInSecs }),
  joinNetwork: (token: string, force: boolean) =>
    invoke<JoinResult>("join_network", { token, force }),
  removeMember: (configPath: string, publicKey: string) =>
    invoke<string>("remove_member", { configPath, publicKey }),
  approveMember: (configPath: string, publicKey: string) =>
    invoke<void>("approve_member", { configPath, publicKey }),
  renameMember: (configPath: string, publicKey: string, newName: string) =>
    invoke<void>("rename_member", { configPath, publicKey, newName }),
  // メンバー招待の発行（ADR-0048）。ホストは端末指名（can_invite）を切り替え、
  // メンバーはデーモン経由でホストへ発行を依頼する（token は秘密情報）
  setMemberCanInvite: (configPath: string, publicKey: string, allowed: boolean) =>
    invoke<void>("set_member_can_invite", { configPath, publicKey, allowed }),
  memberCreateInvite: (
    configPath: string,
    name: string | null,
    expiresInSecs: number | null,
  ) =>
    invoke<MemberInviteResult>("member_create_invite", {
      configPath,
      name,
      expiresInSecs,
    }),
  // DNS 名（ADR-0021、M3-14a）。ホストは任意メンバーを直接、メンバー本人は
  // デーモン経由（接続中のみ・ホストが検証）で変更する
  setMemberDnsName: (configPath: string, publicKey: string, dnsName: string) =>
    invoke<string>("set_member_dns_name", { configPath, publicKey, dnsName }),
  setMyDnsName: (configPath: string, dnsName: string) =>
    invoke<void>("set_my_dns_name", { configPath, dnsName }),
  setHostDnsName: (configPath: string, dnsName: string) =>
    invoke<string>("set_host_dns_name", { configPath, dnsName }),
  // 表示名（ADR-0027、M3-19）。ホストは自分の分を直接、メンバー本人は
  // デーモン経由（接続中のみ・ホストが検証）で変更する。ホストから見た他
  // メンバーの表示名変更は renameMember(peercove-ops)を使う
  setMyDisplayName: (configPath: string, displayName: string) =>
    invoke<void>("set_my_display_name", { configPath, displayName }),
  setHostDisplayName: (configPath: string, displayName: string) =>
    invoke<string>("set_host_display_name", { configPath, displayName }),
  setMemberSubnets: (configPath: string, publicKey: string, subnets: string[]) =>
    invoke<void>("set_member_subnets", { configPath, publicKey, subnets }),
  // ACL: メンバー間通信の遮断組（M3-10、ADR-0018。ホスト設定のみ）
  listAcl: (configPath: string) => invoke<[string, string][]>("list_acl", { configPath }),
  setAcl: (configPath: string, deny: [string, string][]) =>
    invoke<void>("set_acl", { configPath, deny }),
  readAclPolicy: (configPath: string) => invoke<AclPolicySettings>("read_acl_policy", { configPath }),
  writeAclPolicy: (configPath: string, policy: AclPolicySettings) => invoke<void>("write_acl_policy", { configPath, policy }),
  // チャット（ADR-0016、M3-13b/c）。peer 指定で 1:1、group 指定でグループ宛、
  // どちらも null でネットワーク全体宛
  chatSend: (
    configPath: string,
    peer: string | null,
    group: string | null,
    text: string,
  ) => invoke<ChatMessage>("chat_send", { configPath, peer, group, text }),
  chatFetch: (configPath: string, afterSeq: number) =>
    invoke<ChatPage>("chat_fetch", { configPath, afterSeq }),
  // 送信キュー（E-E 3）。失敗した通の再送と、自動再送の取消
  chatResend: (configPath: string, seq: number) =>
    invoke<void>("chat_resend", { configPath, seq }),
  chatCancelSend: (configPath: string, seq: number) =>
    invoke<void>("chat_cancel_send", { configPath, seq }),
  // グループ（M3-13c）。members / add は相手の仮想 IP（自分は不要）
  groupCreate: (configPath: string, name: string, members: string[]) =>
    invoke<Group>("group_create", { configPath, name, members }),
  groupUpdate: (
    configPath: string,
    id: string,
    name: string | null,
    add: string[],
    remove: string[] = [],
  ) => invoke<Group>("group_update", { configPath, id, name, add, remove }),
  groupLeave: (configPath: string, id: string) =>
    invoke<void>("group_leave", { configPath, id }),
  // ファイル送信・受信ボックス（ADR-0015、M3-9b）。chat 付きはチャット内
  // ファイル送信（M3-13d。network / group 宛は peer = null）
  pickFile: () => invoke<string | null>("pick_file"),
  // クリップボードから貼り付けた画像を一時ファイルにして送信できるようにする
  savePastedFile: (name: string, dataBase64: string) =>
    invoke<string>("save_pasted_file", { name, dataBase64 }),
  sendFile: (
    configPath: string,
    peer: string | null,
    path: string,
    chat?: ChatContext,
  ) => invoke<string>("send_file", { configPath, peer, path, chat: chat ?? null }),
  listInbox: (configPath: string) =>
    invoke<InboxItem[]>("list_inbox", { configPath }),
  saveInboxFile: (configPath: string, name: string) =>
    invoke<string | null>("save_inbox_file", { configPath, name }),
  deleteInboxFile: (configPath: string, name: string) =>
    invoke<void>("delete_inbox_file", { configPath, name }),
  // テキストファイルのチャット内プレビュー（M3-13e）
  readTextPreview: (path: string) =>
    invoke<TextPreview>("read_text_preview", { path }),
  // チャットの URL 対応（M3-13e、ADR-0017）
  openLink: (url: string) => invoke<void>("open_link", { url }),
  linkPreview: (url: string) => invoke<LinkPreview>("link_preview", { url }),
  daemonLogs: (afterSeq: number) => invoke<Logs>("daemon_logs", { afterSeq }),
  diagnoseNetwork: (configPath: string) =>
    invoke<DiagnosticReport>("diagnose_network", { configPath }),
  qualityHistory: (configPath: string, sinceUnixMs: number) =>
    invoke<QualityReport>("quality_history", { configPath, sinceUnixMs }),
  listDnsRecords: (configPath: string) =>
    invoke<DnsRecord[]>("list_dns_records", { configPath }),
  // ターゲットは ip（固定）か member（メンバー参照 = IP 自動追随）のどちらか。
  // under で親メンバー配下のサブドメインになる（ADR-0022）
  addDnsRecord: (
    configPath: string,
    name: string,
    target: { ip?: string; member?: string; cname?: string },
    under?: string,
    scheme?: string,
    port?: number,
  ) =>
    invoke<void>("add_dns_record", {
      configPath,
      name,
      ip: target.ip ?? null,
      member: target.member ?? null,
      cname: target.cname ?? null,
      under: under ?? null,
      scheme: scheme ?? null,
      port: port ?? null,
    }),
  removeDnsRecord: (configPath: string, name: string, under?: string | null) =>
    invoke<void>("remove_dns_record", { configPath, name, under: under ?? null }),
  setDnsHealth: (
    configPath: string,
    record: Pick<DnsRecord, "name" | "under">,
    settings: HealthSettings,
  ) => invoke<void>("set_dns_health", {
    configPath,
    name: record.name,
    under: record.under,
    enabled: settings.enabled,
    kind: settings.kind,
    path: settings.path,
    expectedStatus: settings.expectedStatus,
    external: settings.external,
  }),
  checkDnsHealth: (configPath: string) =>
    invoke<void>("check_dns_health", { configPath }),
  readSettings: (configPath: string) =>
    invoke<Settings>("read_settings", { configPath }),
  saveSettings: (configPath: string, update: SettingsUpdate) =>
    invoke<SaveResult>("save_settings", { configPath, update }),
  // 個人メモ(M5 F-1、ADR-0049)。デーモンが DB を所有し IPC で操作する
  memoOp: (op: MemoOp) => invoke<MemoReply>("memo_op", { op }),
  memoExport: (id: string) => invoke<string | null>("memo_export", { id }),
  memoImport: (folderId: string | null) =>
    invoke<number | null>("memo_import", { folderId }),
  // 共有メモ(M5 F-2)。host = 正本 / member = キャッシュ + ホスト依頼
  sharedMemoOp: (configPath: string, op: SharedMemoOp) =>
    invoke<SharedMemoReply>("shared_memo_op", { configPath, op }),
  sharedMemoExport: (configPath: string, id: string) =>
    invoke<string | null>("shared_memo_export", { configPath, id }),
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

/** パスからファイル名だけを取り出す(表示用)。 */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
