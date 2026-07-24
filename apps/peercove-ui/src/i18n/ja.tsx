// 日本語カタログ(表示文言の唯一の正本)。
//
// UI に出す文字列はすべてここに集約する。コンポーネント側にリテラルを残さない
// ことで、将来ほかの言語を足すときは en.tsx を 1 枚コピーして訳すだけで済む
// (追加手順は index.ts のコメント参照)。
//
// 値の種類:
//   - ただの文字列
//   - 数値や名前を差し込むものは関数(例: membersHead: (n) => `メンバー（${n}）`)
//   - <strong>/<code> などの強調を含むものは ReactNode(JSX)。この 1 ファイルに
//     マークアップごと閉じ込めることで、翻訳者はこのファイルだけを見ればよい

import { ReactNode } from "react";

export const ja = {
  // 複数箇所で使う汎用ラベル・ボタン
  common: {
    close: "閉じる",
    cancel: "キャンセル",
    save: "保存",
    saving: "保存中…",
    retry: "再試行",
    loading: "読み込み中…",
    running: "実行中…",
    useAnotherConfig: "別の設定ファイルを使う",
    configFilter: "PeerCove の設定",
    configLabel: "設定",
    configFile: "設定ファイル",
    virtualIp: "仮想 IP",
    delete: "削除",
    add: "追加",
  },

  // トンネルの状態(ipc.ts の stateLabel / バッジ)
  state: {
    idle: "待機中",
    hosting: "ホストとして稼働中",
    joined: "メンバーとして参加中",
    runningCount: (n: number) => (n === 1 ? "稼働中" : `${n} ネットワーク稼働中`),
    daemonDisconnected: "デーモン未接続",
    connectingDaemon: "デーモンに接続しています…",
  },

  // 数値の整形(ipc.ts の format 系ヘルパ)
  format: {
    none: "なし",
    secondsAgo: (n: number) => `${n} 秒前`,
    minutesAgo: (n: number) => `${n} 分前`,
    hoursAgo: (n: number) => `${n} 時間前`,
  },

  header: {
    myDisplayName: (name: string) => `表示名: ${name}`,
    settings: "設定",
    settingsUnavailable:
      "設定ファイルがまだありません（ホストを始めるか参加してください）",
    prefs: "アプリ設定",
    logs: "デーモンのログ",
    // 外観テーマの切替ボタン(M3-6)。ライト ⇄ ダークの 2 状態
    theme: (theme: "light" | "dark") =>
      theme === "light"
        ? "外観: ライト（クリックでダーク）"
        : "外観: ダーク（クリックでライト）",
  },

  // 左サイドバー(M3-15。外枠の刷新)
  sidebar: {
    networks: "ネットワーク",
    memos: "メモ",
    sharedMemos: "共有",
    members: "メンバー",
    chat: "チャット",
    stats: "品質",
    inbox: "受信",
    dns: "DNS",
    subnets: "サブネット",
    acl: "通信制御",
    diagnostics: "診断",
    settings: "設定",
    connected: "接続中",
    disconnected: "未接続",
    theme: (theme: "light" | "dark") =>
      theme === "light"
        ? "外観: ライト（クリックでダーク）"
        : "外観: ダーク（クリックでライト）",
    logs: "ログ",
    version: (v: string) => `v${v}`,
  },

  diagnostics: {
    title: "接続診断",
    lead: "設定と現在の状態を変更せずに確認します。外部の診断サーバーへは送信しません。",
    rerun: "再診断",
    running: "診断しています…",
    noIssues: "確認できた範囲に問題はありません。",
    passed: (count: number) => `正常な項目（${count}）`,
    unknownAction: "詳細な判定に必要な情報がありません。",
    passAction: "問題は見つかりませんでした。",
    evidence: {
      peercoveAclManaged: "PeerCove管理（現在のユーザー＋SYSTEM）",
      modeBitsVerified: "Unix権限を確認済み",
    },
    overall: {
      healthy: "正常",
      attention: "確認が必要",
      problem: "問題があります",
    },
    status: {
      pass: "正常",
      warning: "警告",
      fail: "失敗",
      unknown: "判定不能",
    },
    check: {
      "app.ipc_compatible": {
        summary: "UI とデーモンの IPC は互換です",
        action: "不一致の場合は UI とデーモンを同じ版へ更新してください。",
      },
      "app.version_known": {
        summary: "デーモンのバージョン情報",
        action: "不明な場合はデーモンを更新してください。接続自体は継続できます。",
      },
      "app.peer_compatibility": {
        summary: "接続メンバーのバージョン互換性",
        action: "不明なメンバーは旧版の可能性があります。必要な機能が使えない場合は更新してください。",
      },
      "config.valid": {
        summary: "設定ファイルの読み込みと検証",
        action: "失敗した場合は表示された設定エラーを修正してください。",
      },
      "permissions.secret_files": {
        summary: "秘密ファイルの存在と権限",
        action: "不足ファイルを復元し、他ユーザーから読めない権限にしてください。",
      },
      "tunnel.running": {
        summary: "トンネルの稼働状態",
        action: "停止中ならネットワーク画面から接続してください。",
      },
      "tunnel.interface_ready": {
        summary: "仮想インターフェース",
        action: "名前やアドレスが無い場合は再接続し、管理者権限を確認してください。",
      },
      "tunnel.handshake": {
        summary: "ピアとのハンドシェイク",
        action: "応答が無い場合は接続先、ファイアウォール、ホストの稼働を確認してください。",
      },
      "internet.reachability_evidence": {
        summary: "インターネット到達の証拠",
        action: "外部サーバーを使わないため、実ハンドシェイクが無い場合は判定できません。",
      },
      "dns.zone_available": {
        summary: "内蔵 DNS のゾーン情報",
        action: "台帳が届かない場合は、先にホストとの接続を回復してください。",
      },
      "tunnel.host_removed_member": {
        summary: "ホストからこのメンバーが削除されています",
        action: "切断して、ホスト管理者から新しい招待を受け取ってください。",
      },
      "tunnel.acl": {
        summary: "アクセス制御による遮断",
        action: "意図した遮断かホストの通信制御設定を確認してください。",
      },
      // 共有メモ(M5 F-3、ADR-0049)。DB が無い(共有メモ未使用)場合はチェック自体が出ない
      "memo.db_status": {
        summary: "共有メモデータベースの健全性",
        action: "容量・件数・WAL サイズが上限に近づいています。上限設定の見直しか、不要なメモの整理を検討してください。",
      },
      "memo.cache_status": {
        summary: "共有メモのキャッシュ容量",
        action: "ホストとの再接続時に自動で整理されます。改善しない場合はキャッシュファイルの削除を検討してください。",
      },
    },
  },

  quality: {
    title: "通信品質",
    description: "RTT・パケット損失・経路の推移を、この端末だけに7日間保存します。",
    peer: "対象メンバー",
    period: "表示期間",
    summary: "品質の概要",
    latestRtt: "最新 RTT",
    averageRtt: "平均 RTT",
    p95Rtt: "P95 RTT",
    loss: "損失率",
    lastConnected: "最終接続",
    rttChart: "RTT の推移",
    lossChart: "パケット損失",
    route: "経路",
    table: "数値データ",
    time: "時刻",
    state: "接続状態",
    jitter: "ジッター",
    transfer: "転送量",
    direct: "直接",
    relay: "ホスト経由",
    trying: "確立中",
    connected: "接続中",
    disconnected: "制御接続なし",
    unmeasured: "測定対象外",
    loading: "品質履歴を読み込んでいます…",
    empty: "品質データはまだありません。接続後、最初の測定まで数秒かかります。",
    noRtt: "この期間に RTT の測定値はありません。",
    noLoss: "この期間に損失率を計算できる Ping はありません。",
    gaps: "制御接続がない時間は線をつながず、欠測として表示します。",
    lossNote: "制御接続断は 100% 損失とは数えません。",
    rttAria: "時間ごとの平均 RTT 折れ線グラフ。数値は下の表でも確認できます。",
    lossAria: "時間ごとのパケット損失率の棒グラフ。数値は下の表でも確認できます。",
    switches: (count: number) => `経路切替 ${count} 回`,
    corrupt: (count: number) => `読み取れない履歴 ${count} 行を安全に読み飛ばしました。`,
  },

  footer: (
    <>
      wintun.dll © WireGuard LLC — Prebuilt Binaries License の下で無改変同梱
      （インストール先の <span className="mono">wintun-LICENSE.txt</span>{" "}
      を参照）。
    </>
  ),

  // アプリ全体の設定(M3-13e)。このマシンだけに効く(localStorage)
  prefs: {
    title: "アプリ設定",
    notifications: "OS 通知を出す",
    notificationsHint:
      "メンバーの参加・切断、チャットの新着、ファイルの受信完了の通知です。オフにしても未読バッジや受信タブの表示は変わりません。",
    linkPreview: "チャットの URL のプレビューを表示する",
    linkPreviewHint:
      "URL を含むメッセージを表示したとき、この端末がそのページへ情報（タイトルや画像）を取りに行きます。相手のサイトにあなたの IP アドレスが伝わるのが気になる場合はオフにしてください。",
    qualityAlerts: "通信品質が低下したときに通知する",
    qualityAlertsHint: "損失率が3分続けて閾値を超えた場合と、直接経路からホスト経由へ切り替わった場合に通知します。既定はオフです。",
    qualityLossThreshold: "損失率の閾値",
    note: "この設定はこのマシンにだけ保存されます（即時反映）。",
  },

  update: {
    title: "更新とバージョン",
    enabled: "新しいバージョンを自動で確認する",
    enabledHint:
      "1日1回、GitHub Releasesへ最新版だけを確認します。設定やネットワーク情報は送信しません。",
    uiVersion: "アプリ",
    daemonVersion: "デーモン",
    latestVersion: "最新版",
    unknown: "不明",
    checkNow: "今すぐ確認",
    checking: "確認中…",
    disabledHint: "更新の自動確認はオフです。確認するには上のスイッチを有効にしてください。",
    current: "最新バージョンを使用しています。",
    available: (version: string) => `v${version} を利用できます。`,
    openRelease: "リリースページを開く",
    sidebar: (version: string) => `v${version} · 更新あり`,
    memberVersion: (version: string) => `PeerCove v${version}`,
    memberVersionUnknown: "PeerCove バージョン不明",
    memberPlatformTitle: "この端末の OS（接続時に申告）",
  },

  daemonOutdated: {
    title: "デーモンの更新が必要です",
    body: "動いているデーモンが古いバージョンのため、状態を正しく表示できません（トンネルが稼働中でも「停止中」に見えます）。サービスを入れ替えてください:",
    windows:
      "管理者 PowerShell: Stop-Service peercove-daemon → 新しい peercove.exe で daemon service-uninstall → daemon service-install",
    linux:
      "sudo systemctl stop peercove-daemon → 新しいバイナリを配置 → sudo systemctl start peercove-daemon",
  },

  daemonUnreachable: {
    title: "デーモンに接続できません",
    body: "トンネルの操作には管理者権限のデーモンが必要です。ターミナルで次を実行してください:",
    command: "peercove daemon run",
    platforms: "Windows は管理者ターミナル、Linux は sudo で実行します。",
    details: "詳細",
  },

  // ネットワーク一覧(M3-0c)
  networks: {
    listHead: "ネットワーク",
    empty: (
      <>
        まだネットワークがありません。自分が<strong>中心（ホスト）になって作る</strong>か、
        招待トークンで<strong>既存のネットワークに参加</strong>してください。
      </>
    ),
    running: "稼働中",
    stopped: "停止中",
    roleHost: "ホスト",
    roleMember: "メンバー",
    membersOnline: (n: number) => `オンライン ${n} 人`,
    open: "開く",
    connect: "接続",
    connecting: "接続中…",
    disconnecting: "切断中…",
    removedBadge: "削除されました",
    external: "（一覧外の設定で稼働中）",
    back: "← 一覧へ",
    settings: "設定",
    delete: "削除",
    deleteTitle: "ネットワークを削除",
    deleteConfirm: "削除する",
    deleteMessage: (name: string): ReactNode => (
      <>
        <p>
          ネットワーク <strong>{name}</strong> の設定を削除します。
        </p>
        <p className="muted">
          鍵ファイルも一緒に消えるため、元に戻せません。再参加には新しい
          招待トークンが必要です。ホストしていた場合、メンバーは接続できなくなります。
        </p>
      </>
    ),
    addHost: "新しくホストする",
    addJoin: "招待トークンで参加する",
    addClose: "閉じる",
    nameLabel: "ネットワーク名",
    nameHint:
      "半角英数字とハイフン。フォルダ名と、将来の DNS アドレス（〜.peercove.internal）に使われます。",
    namePlaceholder: "home",
    create: "作成してホスト開始",
    creating: "作成中…",
  },

  start: {
    intro: (
      <>
        このネットワークの<strong>中心（ホスト）になる</strong>か、
        既存のネットワークに<strong>参加する</strong>かを選んでください。
        どちらも「新しく始める」と「保存済みの設定で再開する」を選べます。
      </>
    ),
    locateError: "設定の場所を特定できません",
    host: {
      title: "ホストを始める",
      lead: "あなたの PC がネットワークの中心になります。参加者を招待できます。",
      stateLabel: "状態",
      stateExisting: "既存のネットワークを使います",
      stateNew: "新しく作成します（鍵とアドレスを自動生成）",
      upnp: (
        <>
          ルーターのポートを自動で開ける（UPnP）
          <small className="muted"> — 別ネットワークの人を招くとき必要</small>
        </>
      ),
      creating: "ネットワークを作成中…",
      starting: "トンネルを開始中…",
      start: "開始",
      createAndStart: "作成して開始",
      note: "トンネルの操作には管理者権限のデーモンが必要です（このアプリ自体は通常権限で動きます）。",
    },
    join: {
      title: "参加する",
      lead: "ほかの人がホストするネットワークにメンバーとして加わります。",
      savedHead: "保存済みの設定で再接続",
      reconnect: "前回のネットワークに再接続",
      starting: "トンネルを開始中…",
      creating: "参加設定を作成中…",
      toggleClose: "新しい招待での参加を閉じる",
      toggleOpen: "別のネットワークに新しく参加する（招待トークン）",
      newHead: "招待トークンで新しく参加",
      tokenHint: (
        <>
          ホストから受け取った招待トークン（<code>pcv1.</code> で始まる
          文字列）を貼り付けてください。
        </>
      ),
      tokenPlaceholder: "pcv1.…",
      overwrite: "既存の参加設定を上書きする",
      submit: "参加する",
    },
  },

  tunnel: {
    removedTitle: "ホストから削除されました",
    removedBody:
      "このネットワークからあなたは削除されました。通信はすでに遮断されています。「切断」してから、必要なら新しい招待トークンで参加し直してください。",
    rejectedTitle: "接続が拒否されました",
    rejectedBadge: "参加拒否",
    rejectedAction:
      "この参加設定では再接続できません。「切断」してから、ホストが発行した新しい招待トークンで参加し直してください。",
    disconnectConfirm: "切断する",
    disconnect: "切断",
    back: "一覧へ戻る",
    connected: "接続中",
    configFileLabel: "設定ファイル",
    membersHead: (n: number) => `メンバー（${n}）`,
    // 詳細ヘッダー直下の統計カード(M3-15)
    overview: {
      virtualIp: "仮想IP",
      online: "オンライン",
      onlineCount: (n: number) => `${n}人`,
      rate: "転送速度",
    },
    // メンバー表の見出し(M3-15)
    table: {
      role: "ロール",
      dnsName: "DNS 名",
      virtualIp: "仮想IP",
      rate: "転送速度",
      rtt: "遅延",
      actions: "アクション",
      copyIp: "仮想IP をコピー",
      chat: "チャットを開く",
    },
    // DNS サービスカード(M3-15、ADR-0023 の URL を活用)
    service: {
      head: "DNS サービス",
      hint: "スキームを設定したカスタム DNS レコードは、ここからワンクリックで開けます（全メンバーに表示されます）。",
      copyUrl: "URLをコピー",
      copied: "コピーしました",
      openTitle: "既定のブラウザで開く",
    },
    // ネットワーク詳細のタブ(M3-6)
    tabs: {
      chat: "チャット",
      stats: "品質",
      inbox: "受信",
    },
    invite: "メンバーを招待",
    ledgerPending: "台帳をまだ受信していません（接続直後は数秒かかります）。",
    removeNotice: (name: string) =>
      `${name} を削除しました。約 10 秒でトンネルから外れます。`,
    peers: {
      head: "ピア統計",
      publicKey: "公開鍵",
      endpoint: "エンドポイント",
      lastHandshake: "最終ハンドシェイク",
      rtt: "RTT",
      rate: "速度",
      rx: "受信",
      tx: "送信",
      notConnected: "(未接続)",
      rttTitle: "トンネル内のコントロールチャネルで測った往復時間（線は直近約 90 秒の推移）",
      rateTitle: "受信+送信の合計速度（線は直近約 90 秒の推移）",
      empty: "まだ統計がありません（接続直後は数秒かかります）。",
    },
    remove: {
      title: "メンバーを削除",
      confirm: "削除する",
      message: (who: string): ReactNode => (
        <>
          <p>
            <strong>{who}</strong> をネットワークから削除します。
          </p>
          <p className="muted">
            本人へ通知され、約 10 秒でトンネルから外れます。渡した招待トークンも
            使えなくなります。もう一度参加してもらうには招待をやり直してください。
          </p>
        </>
      ),
    },
    member: {
      online: "オンライン",
      offline: "オフライン",
      noName: "(名前なし)",
      rttTitle: "トンネル内の往復時間",
      rateTitle: "このメンバーとの転送速度（受信+送信、直近約 90 秒の推移）",
      rename: "表示名を変更",
      // 表示名の変更(ADR-0027、M3-19)。自分の行から本人が変更できる
      displayRenamed: "表示名を変更しました。数秒で全員に反映されます。",
      // DNS 名の分離(ADR-0021、M3-14a)
      editDns: "DNS 名を変更（英数字とハイフンのみ。表示名とは独立）",
      dnsRenamed: "DNS 名を変更しました。数秒で全員に反映されます。",
      remove: "削除",
      self: "自分",
      selfTitle: "このマシン自身です",
      // メンバー間の経路(M3-4、ADR-0013)
      route: {
        direct: "直接",
        trying: "確立中…",
        relay: "中継",
        aclRelay: "ACLにより中継",
        aclTitle: "細粒度ACLを確実に適用するためホスト経由に固定されています。隣の文字列はルールIDです。",
        title:
          "このメンバーとの通信経路。「直接」は相手と直接接続中、「中継」はホスト経由、「確立中…」は直接接続を試しているところです。",
      },
      // ACL(M3-10、ADR-0018)
      blocked: "通信不可",
      blockedTitle:
        "ホストの通信制御により、このメンバーとの通信は遮断されています",
      inviteStatus: {
        legacy: "既存",
        pending: "未使用",
        joined: "参加済み",
        awaiting_approval: "承認待ち",
        expired: "期限切れ",
        clock_invalid: "時刻エラー",
      },
      approve: "承認",
      approved: "参加端末を承認しました。隔離は数秒以内に解除されます。",
      inviteExpires: (value: string) => `招待期限: ${value}`,
      // メンバー詳細ページ(ADR-0048)
      detail: "詳細",
      detailTitle: (name: string) => `${name} の詳細`,
      detailName: "表示名",
      detailRole: "役割",
      detailDns: "DNS 名",
      detailOs: "OS",
      detailVersion: "アプリ",
      detailKey: "公開鍵",
      detailState: "状態",
      detailRoute: "経路",
      detailInvite: "招待の状態",
      detailSubnets: "公開サブネット",
      invitedBy: "招待者",
      invitedByHost: "ホスト",
      // メンバーによる招待発行の端末指名(ADR-0048)
      canInviteLabel: "この端末にメンバー招待の発行を許可",
      canInviteHint:
        "許可すると、この端末から新しいメンバーを招待できます（発行はホストの記録と全体チャットに残ります）。ネットワーク設定の「メンバーによる招待発行を許可」が OFF の間は無効です。",
      canInviteUpdated: "招待発行の許可を変更しました。数秒で反映されます。",
    },
    // 直接通信の説明(M3-4)。外部 IP の共有について明記する(ADR-0013 条件 3)
    directNote:
      "メンバー同士は可能なら直接通信します(速く・ホスト回線の負荷なし)。このとき、あなたのグローバル IP アドレスが同じネットワークのメンバーに共有されます。使いたくない場合は「設定」で直接通信をオフにしてください。",
  },

  // チャット(M3-13b/c、ADR-0016)
  chat: {
    all: "全体",
    allNote: "ネットワーク全員に届きます（送信時にオンラインのメンバーのみ）",
    empty: "まだメッセージはありません。",
    noMessages: "メッセージなし",
    pin: "ピン留め（常に上に表示）",
    unpin: "ピン留めを解除",
    mute: "通知をミュート",
    unmute: "ミュートを解除",
    moveUp: "上に移動",
    moveDown: "下に移動",
    searchPlaceholder: "メッセージを検索",
    searchEmpty: "見つかりませんでした",
    previewSelf: "自分: ",
    placeholder:
      "メッセージを入力（Enter で送信、Shift+Enter で改行、ファイルは貼り付け可）",
    offline: "オフラインのメンバーには送れません",
    blocked: "ホストの通信制御により、このメンバーには送れません",
    left: "このメンバーは現在ネットワークにいません（履歴のみ）",
    send: "送信",
    failed: "送信失敗",
    // 送信キュー(E-E 3)
    sendingState: "送信中…",
    retrying: "未送信（自動再送します）",
    resend: "再送",
    cancelSend: "取消",
    // グループ(M3-13c)
    groupCreate: "グループ作成",
    groupTitle: "新しいグループ",
    groupManage: "管理",
    groupNameLabel: "グループ名",
    groupNamePlaceholder: "例: 開発チーム",
    groupMembersLabel: "メンバーを選ぶ",
    groupAddLabel: "メンバーを追加",
    groupRemoveLabel: "メンバーを外す",
    groupMembersHead: "メンバー",
    groupNoCandidates: "追加できるメンバーがいません。",
    groupCount: (n: number) => `${n} 人`,
    groupNote:
      "グループの情報はメンバー同士で直接共有されます。オフラインのメンバーには、オンラインに戻ったときに自動で届きます。",
    create: "作成",
    save: "保存",
    leave: "退出",
    leaveTitle: "グループから退出",
    leaveConfirm: (name: string) =>
      `「${name}」から退出しますか？　これまでの履歴はこの端末に残ります。`,
    leftGroup: "このグループからは退出済みです（履歴のみ）",
    unknownGroup: "グループ（同期中）",
    groupPending:
      "グループ情報の受信待ちです（メンバーがオンラインになると自動で届きます）",
    // チャット内ファイル送信 + ドラッグ&ドロップ(M3-13d)
    attach: "ファイルを送る",
    filePreview: (name: string) => `📎 ${name}`,
    fileFailed: "転送失敗",
    fileStarted: (count: number) =>
      count === 1
        ? "ファイルの送信を開始しました。"
        : `${count} 件のファイルの送信を開始しました。`,
    dropTitle: "ファイルを送信",
    dropMessage: (dest: string) => `「${dest}」へ送信しますか？`,
    dropHint: (dest: string) => `ドロップして「${dest}」へ送信`,
    // テキストファイルのプレビュー(M3-13e)
    textOpen: "クリックで全文を表示",
    textTruncated: "ファイルが大きいため先頭のみ表示しています。",
  },

  // 共有オブジェクト参照 `@memo:id` のカード(M5 F-5 Stage 4、ADR-0052 決定 1)
  sharedRef: {
    loading: "読み込み中…",
    inaccessible: "アクセスできないメモ",
  },

  // ファイル送信・受信ボックス(M3-9b、ADR-0015)
  transfer: {
    head: "転送",
    // 一覧の行頭に出す向き
    direction: (d: "send" | "recv") => (d === "send" ? "送信" : "受信"),
    done: "完了",
    failed: (reason: string) => `失敗: ${reason}`,
    progress: (transferred: string, size: string) => `${transferred} / ${size}`,
    // ファイル送信ダイアログ(M3-13e: 宛先をチェックボックスで選ぶ)
    sendButton: "📤 ファイル送信",
    dialogTitle: "ファイル送信",
    fileLabel: "送るファイル",
    pick: "ファイルを選ぶ",
    noFile: "（未選択）",
    recipientsLabel: "送る相手（複数選べます）",
    noCandidates: "送れるメンバーがいません。",
    dialogNote:
      "オフラインのメンバーには送れません。進捗は受信タブの転送一覧に宛先ごとに出ます。",
    sendTo: (n: number) => (n === 0 ? "送信" : `${n} 人へ送信`),
    startedMany: (n: number) =>
      n === 1
        ? "送信を開始しました（進捗はこのタブに出ます）。"
        : `${n} 人への送信を開始しました（進捗はこのタブに出ます）。`,
  },
  inbox: {
    head: "受信ボックス",
    note: "受け取ったファイルは自動でここに入ります。「保存」で好きな場所へ移動してください。",
    empty: "受信したファイルはありません。",
    from: (who: string) => `${who} から`,
    save: "保存",
    delete: "削除",
    savedTo: (path: string) => `保存しました: ${path}`,
    deleted: (name: string) => `${name} を削除しました。`,
  },

  // サブネット共有(M3-7b、ADR-0014 → M3-16 でページ化)
  subnet: {
    pageTitle: "サブネット（背後 LAN の共有）",
    intro:
      "あるメンバーの背後にある LAN(自宅の NAS・プリンタ・別の PC など)を、ネットワークの全員から使えるようにします。設定できるのはホストだけで、変更は約 10 秒で全員に反映されます。",
    memberLabel: (name: string) => `${name} が公開する LAN`,
    placeholder: "192.168.10.0/24",
    hint: "IP レンジ(CIDR)で指定します。例: 192.168.10.0/24。スペース区切りで複数指定でき、空欄にして保存すると公開を解除します。",
    empty:
      "公開できるメンバーがまだいません(ホスト以外のメンバーが参加すると、ここで設定できます)。",
    note: "LAN を公開できる(転送役になれる)のは Linux のメンバーだけです(Windows は届く側専用)。公開すると、その LAN は全メンバーから到達でき、LAN 側の機器にはアクセス元がそのメンバーの端末に見えます(NAT)。",
    save: "保存",
    saved: "保存しました。約 10 秒で全員に反映されます。",
    badgeTitle:
      "このメンバーが公開している背後 LAN のサブネット（全メンバーから到達可能）",
  },

  // 通信制御 ACL(M3-10、ADR-0018)
  acl: {
    button: "🚦 通信制御",
    title: "メンバー間の通信制御",
    intro:
      "「遮断する」にした 2 人の間では、チャット・ファイル送信を含むすべての通信ができなくなります（ホストとの通信は遮断できません）。変更は約 5 秒で全員に反映されます。",
    needTwo: "メンバーが 2 人以上になると設定できます。",
    block: "遮断する",
    blockedTag: "遮断中",
    introV2: "上から最初に一致したルールを適用します。方向は新規通信の開始方向で、許可された逆方向通信の応答は通ります。メンバー／グループ／サブネット、プロトコル、ポートを指定でき、変更は約5秒で反映されます。",
    defaultAction: "どのルールにも一致しない通信",
    allow: "許可",
    deny: "拒否",
    rules: "ルール",
    noRules: "個別ルールはありません。既定の動作が使われます。",
    order: "順序",
    action: "動作",
    source: "送信元",
    destination: "宛先",
    protocol: "プロトコル",
    ports: "宛先ポート",
    state: "状態",
    actions: "操作",
    allPorts: "すべて",
    enabled: "有効",
    disabled: "無効",
    moveUp: "1つ上へ移動",
    moveDown: "1つ下へ移動",
    addRule: "ルールを追加",
    relayWarning: "このルールに関係するメンバー間通信は、評価を確実にするためホスト中継へ切り替わります。",
    any: "すべて",
    member: "メンバー",
    group: "グループ",
    subnet: "サブネット",
    service: "DNSサービス",
    targetRequired: "送信元と宛先を選択してください。",
    groups: "メンバーグループ",
    groupName: "新しいグループ名",
    addGroup: "グループを追加",
    people: "人",
  },

  // DNS 管理画面(M3-1c、ADR-0022/0023 でカスタムレコード拡張)
  dns: {
    title: "DNS",
    button: "DNS",
    intro: (
      <>
        メンバーは <code>DNS名.ネットワーク名.peercove.internal</code>{" "}
        で呼べます。DNS 名はメンバー一覧の ✎ から変更できます。
      </>
    ),
    autoHead: "自動レコード（メンバー）",
    autoEmpty: "台帳をまだ受信していません。",
    customHead: "カスタムレコード",
    customEmpty: "カスタムレコードはありません。",
    customNote:
      "ホストだけが追加・削除できます。約 10 秒で全メンバーに配布されます。" +
      "「この左側」にマシンを選ぶと、そのマシンのサブドメインになり、マシンの名前変更にも自動で追随します。",
    customNoteMember: "カスタムレコードはホストが管理します（ここでは閲覧のみ）。",
    // ドメイン名の入力(ADR-0024)
    domainLabel: "ドメイン名",
    prefixPlaceholder: "* / web / api など（任意）",
    baseFree: "自由入力",
    baseFreePlaceholder: "app",
    baseMember: (name: string) => `${name}（マシン）`,
    wildcardHint:
      "左側は空でも、web や api.v2 のようにドットで区切っても構いません。先頭に * を付けると、その 1 ラベルが任意になるワイルドカード（例: *.app）になります。候補からマシンを選ぶと、そのマシンのサブドメインにできます。",
    baseIsMachine:
      "このマシンのサブドメインになります（マシンの名前変更にも自動で追随）。左側（* や web など）を入力してください。",
    previewLabel: "登録される名前",
    // 転送先(ADR-0024)
    forwardLabel: "転送先（このドメインが指す先）",
    forwardIp: "IP アドレスを指定",
    forwardCname: "ドメイン（CNAME）を指定",
    forwardMember: (name: string) => `マシン: ${name}`,
    cnamePlaceholder: "docs.example.com",
    cnameHint:
      "別のドメインの別名（CNAME）にします。外部ドメイン（example.com など）も指定できます。閲覧側はこのドメインを引くと、指定先へたどり直します。",
    cnameTag: "→ 別ドメイン",
    ipPlaceholder: "10.68.1.50",
    schemeLabel: "スキーム（任意）",
    schemePlaceholder: "http",
    portLabel: "ポート（任意）",
    portPlaceholder: "8080",
    serviceHint:
      "スキームを指定すると URL を組み立てて全メンバーに表示します。ポートだけ指定した場合は DNS名:ポートで表示します。",
    targetOf: (name: string) => `→ ${name}`,
    openTitle: "既定のブラウザで開く",
    brokenRef: "参照先なし",
    add: "追加",
    adding: "追加中…",
    remove: "削除",
    copy: "コピー",
    copyUrl: "URL をコピー",
    copied: "コピーしました",
    health: {
      checkNow: "今すぐ確認",
      checking: "確認を開始しました…",
      settings: "ヘルスチェック設定",
      dialogTitle: "サービスのヘルスチェック",
      enabled: "このサービスを定期的に確認する",
      enabledHint: "ホストから60秒ごとに確認します。失敗してもDNSの回答は停止しません。",
      external: "外部CNAMEへの接続を許可する",
      externalHint: "有効にすると、ホストの外部IPや確認時刻が転送先サービスへ伝わる可能性があります。",
      kind: "確認方式",
      tcp: "TCP接続のみ",
      httpHead: "HTTP HEAD（本文は取得しない）",
      path: "確認パス",
      expected: "期待ステータス",
      notChecked: "未確認",
      checked: (time: string) => "最終確認 " + time,
      openWarning: "現在は応答を確認できません。URLは引き続き開けます。",
      status: {
        healthy: "稼働中",
        unhealthy: "応答なし",
        unknown: "未確認",
        disabled: "確認オフ",
      },
      reason: {
        not_checked: "まだ確認していません",
        offline: "対象メンバーがオフラインです",
        timeout: "3秒以内に応答がありません",
        connection_failed: "接続できません",
        name_resolution_failed: "転送先を解決できません",
        unexpected_status: "期待したHTTP状態ではありません",
        disabled: "定期確認は無効です",
      },
    },
  },

  invite: {
    resultTitle: (name: string) => `${name} さんの招待`,
    warn: (
      <>
        このトークンは<strong>この画面でしか表示されません</strong>。
        本人だけに渡し、受け渡し後は削除してください。
      </>
    ),
    allocatedIp: "割当 IP",
    endpoints: "接続先候補",
    psk: "事前共有鍵",
    expires: "有効期限",
    never: "無期限",
    yes: "あり",
    no: "なし",
    copied: "コピーしました",
    copy: "トークンをコピー",
    // 招待ディープリンク(M3-5)
    copyLink: "参加リンクをコピー",
    copyLinkHint:
      "参加リンク(peercove://…)は、PeerCove がインストール済みの相手ならクリックだけで参加フォームが開きます。未インストールの相手にはトークンを渡してください。",
    resultNote:
      "同じ LAN のメンバーは LAN 側の候補、別ネットワークのメンバーは外部の候補で接続します。取り消すときはメンバー一覧から削除してください。",
    formTitle: "メンバーを招待",
    nameLabel: "名前（省略すると自動で付きます）",
    namePlaceholder: "alice",
    externalLabel: "外部の接続先（LAN 外・スマホのモバイル回線から招く場合）",
    externalHint:
      "LAN 内の接続先は自動で入ります。UPnP が有効なら外部 IP も自動で追加されます。" +
      "UPnP が使えないときは「グローバルIP:待受ポート」を入力し、ルーターでその UDP ポートをこの PC へ転送してください。",
    externalPlaceholder: "203.0.113.5:51822",
    pskLabel: "事前共有鍵（PSK）も発行する",
    expiryLabel: "招待の有効期限",
    expiryHour: "1 時間",
    expiryDay: "1 日",
    expiryWeek: "7 日（推奨）",
    expiryMonth: "30 日",
    expiryHint: "期限切れ後は、まだ参加していない端末からの接続をホストが拒否します。",
    issuing: "発行中…",
    issue: "招待を発行",
    // メンバーによる招待発行(ADR-0048)
    memberFormNote:
      "ホストに依頼して招待を発行します。発行はホストの記録と全体チャットのお知らせに残ります。有効期限は最長 7 日です。",
    memberResultNote:
      "接続先の候補はホストが自動で決めます。取り消しはホストに依頼してください。",
  },

  logs: {
    title: "デーモンのログ",
    level: "表示レベル",
    levelOption: (level: string) => `${level} 以上`,
    follow: "最新行を追う",
    clear: "表示をクリア",
    dropped: (n: number) => `バッファから溢れた ${n} 行は失われています。`,
    empty: "まだログがありません。",
    emptyForLevel: "このレベルに該当する行がありません。",
    footer: (
      <>
        デーモンが <code>--log-level</code> で絞っている場合、ここにもそれより
        詳しい行は出ません。詳細を見るにはデーモンを
        <code> --log-level debug</code> で起動し直してください。
      </>
    ),
  },

  backup: {
    title: "バックアップと復元",
    description: "設定・鍵・DNS・ACL・グループを、パスフレーズで暗号化して保存します。チャット、受信ファイル、品質履歴、ログ、診断結果は含みません。",
    createTitle: "バックアップを作成",
    restoreTitle: "バックアップから復元",
    network: "ネットワーク",
    passphrase: "パスフレーズ",
    confirm: "パスフレーズ（確認）",
    passphraseHint: "12文字以上で入力してください。パスフレーズは保存されません。",
    passphraseLength: "パスフレーズは12文字以上にしてください。",
    passphraseMismatch: "確認用のパスフレーズが一致しません。",
    create: "暗号化して保存",
    created: (path: string) => `バックアップを保存しました: ${path}`,
    choose: "バックアップを選択",
    preview: "内容を確認",
    role: "役割",
    host: "ホスト",
    member: "メンバー",
    sourceOs: "作成元OS",
    categories: "含まれる項目",
    // 共有メモの同梱(M5 F-3、ADR-0049)。ホスト設定を選んでいるときだけ表示
    includeMemos: "共有メモを含める",
    // categories(Rust 側 crates/peercove-ops/src/backup.rs)の表示名
    category: {
      network_config: "ネットワーク設定",
      keys: "鍵",
      groups: "グループ",
      memos: "共有メモ",
      dns: "DNS レコード",
      acl: "通信制御(ACL)",
      invite_metadata: "招待の記録",
    },
    restoreName: "復元後の名前",
    replace: "同名の既存ネットワークを置き換える",
    replaceHint: "置き換える場合も、復元内容の検証が完了してから切り替えます。稼働中のネットワークは先に切断してください。",
    restore: "復元する",
    restored: "ネットワークを復元しました。",
    rotateRecommendation: "別の端末へ復元したメンバー設定は、接続後にデバイス鍵を更新することを推奨します。",
  },

  settings: {
    title: "設定",
    mtuInteger: "MTU には整数を入力してください",
    portInteger: "待受ポートには整数を入力してください",
    savedRestart: "保存しました。数秒以内に自動で再接続して反映します。",
    savedLive: "保存しました。数秒でトンネルに反映されます。",
    interface: "インターフェース",
    // 表示名・DNS 名の変更はメンバー一覧へ一元化した(ADR-0027、M3-19)
    nameMovedHint:
      "表示名・DNS 名は「メンバー」一覧の自分の行にある ✎ から変更できます。",
    portLabel: "待受ポート（UDP）",
    portHint: (isMember: boolean, defaultPort: number) =>
      ` — 空欄なら${isMember ? "OS 任せ" : `既定の ${defaultPort}`}`,
    mtuLabel: "MTU",
    mtuHint: (defaultMtu: number) =>
      ` — 既定 ${defaultMtu}。回線によっては下げると安定します`,
    hostEndpointLabel: "ホストのエンドポイント",
    hostEndpointHint: " — ホストの IP:ポート。引っ越し後の付け替えに使います",
    hostEndpointPlaceholder: "203.0.113.5:51820",
    // メンバー間直接通信のトグル(ADR-0013 条件 2)
    directLabel: "メンバーと直接通信する（推奨）",
    directHint:
      "— 可能なら相手と直接つなぎます（速く・ホスト回線の負荷なし）。あなたのグローバル IP がネットワーク内のメンバーに共有されます。オフにすると常にホスト経由。約 10 秒で反映されます",
    inviteApprovalLabel: "新しい端末の参加を承認する",
    inviteApprovalHint: "— 承認まではコントロール通信以外を隔離します",
    // メンバーによる招待発行(ADR-0048)
    memberInvitesLabel: "メンバーによる招待発行を許可",
    memberInvitesHint:
      "— 許可した端末（メンバー詳細で指名）だけが招待を発行できます。OFF にすると全端末で無効になります",
    // 受信ファイルサイズ上限(ADR-0015、M3-9)
    maxFileLabel: "受信ファイルの上限（MB）",
    maxFileHint: (defaultMb: number) =>
      ` — 既定 ${defaultMb}。0 で無制限。これを超えるファイルの受信は拒否されます（約 10 秒で反映）`,
    maxFileInteger: "受信ファイルの上限には 0 以上の整数を入力してください",
    restartHint: (isMember: boolean) =>
      `待受ポート・MTU${
        isMember ? "・ホストのエンドポイント" : ""
      }は、保存すると自動でトンネルを作り直して反映されます（数秒以内）。`,
    // デバイス鍵ローテーション(ADR-0020、M3-11)
    rotateKeyLabel: "デバイス鍵",
    rotateKeyHint:
      "この端末で鍵を作り直し、ホストへ登録し直します。参加直後に一度自動で行われるため、通常は不要です（招待トークンが漏れた心配があるときなどに）。",
    rotateKeyButton: "鍵を更新",
    rotateKeyConfirm:
      "デバイス鍵を更新しますか？ 切り替え時に数秒間通信が途切れます。",
    rotateKeyRequested:
      "鍵の更新を要求しました。完了すると数秒間の再接続が発生します（結果はログに出ます）。",
  },

  // OS 通知(notify.ts)
  notify: {
    approvalTitle: "PeerCove: 新しい参加要求",
    joinedTitle: "メンバーが参加しました",
    leftTitle: "メンバーが切断しました",
    body: (name: string, ip: string, network: string) =>
      `${name}（${ip}）— ${network}`,
    // ファイル受信(M3-9b)
    fileTitle: "ファイルを受信しました",
    fileBody: (file: string, from: string, network: string) =>
      `${file} — ${from} から（${network}）`,
    // チャット新着(M3-13b)。LINE と同じく本文を通知に出す
    chatTitle: (from: string, network: string) => `${from}（${network}）`,
    chatBody: (text: string, isAll: boolean) =>
      isAll ? `[全体] ${text}` : text,
    // 共有メモのコメント・メンション(M5 F-5 Stage 3、ADR-0052 決定 4・5)。
    // 通知はローカル表示でありログではないため、メモタイトル・コメント本文を出してよい
    commentTitle: (from: string, memoTitle: string) =>
      `${from} が「${memoTitle}」にコメントしました`,
    mentionTitle: (from: string, memoTitle: string) =>
      `${from} があなたをメンションしました（「${memoTitle}」）`,
    // メモのリマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6)
    reminderTitle: (memoTitle: string) => `⏰ メモのリマインダー: ${memoTitle}`,
  },

  // 個人メモ(M5 F-1、ADR-0049)
  memo: {
    title: "メモ",
    newMemo: "新規メモ",
    searchPlaceholder: "検索（タイトル・本文）",
    scopeActive: "メモ",
    scopeArchived: "アーカイブ",
    scopeTrash: "ゴミ箱",
    sortLabel: "並び順",
    sortUpdated: "更新順",
    sortCreated: "作成順",
    sortTitle: "タイトル順",
    folders: "フォルダー",
    allMemos: "すべてのメモ",
    noFolder: "フォルダーなし",
    newFolder: "新しいフォルダー",
    folderNamePlaceholder: "フォルダー名",
    renameFolder: "フォルダー名を変更",
    renamePrompt: "新しいフォルダー名",
    deleteFolder: "フォルダーを削除",
    folderDeleteConfirm: (name: string) =>
      `フォルダー「${name}」を削除しますか？（中のメモは「フォルダーなし」へ移動します）`,
    empty: "メモがありません。「＋ 新規メモ」から作成できます",
    emptyTrash: "ゴミ箱は空です",
    untitled: "無題",
    selectPrompt: "左の一覧からメモを選ぶか、新規作成してください",
    loadFailed:
      "メモを読み込めません。デーモンが起動しているか、バージョンが古くないか確認してください",
    inTrash: "ゴミ箱のメモ（読み取り専用）",
    pin: "ピン留め",
    unpin: "ピン留めを外す",
    archive: "アーカイブへ移動",
    unarchive: "アーカイブから戻す",
    duplicate: "複製",
    toTrash: "ゴミ箱へ移動",
    restore: "復元",
    deleteForever: "完全削除",
    deleteForeverConfirm: "このメモを完全に削除しますか？（元に戻せません）",
    emptyTrashAction: "ゴミ箱を空にする",
    emptyTrashConfirm:
      "ゴミ箱のメモをすべて完全削除しますか？（元に戻せません）",
    import: "テキスト取り込み",
    importNote:
      "UTF-8 の .txt を個人メモとして取り込みます（ファイル名がタイトルになります）",
    imported: (n: number) => `${n} 件のテキストを取り込みました`,
    exportNote:
      "本文を UTF-8 のテキストとして保存します（タグや履歴は含まれません）",
    exported: (path: string) => `保存しました: ${path}`,
    modeEdit: "編集",
    modePreview: "プレビュー",
    modeSplit: "分割",
    titlePlaceholder: "タイトル",
    bodyPlaceholder: "本文（Markdown が使えます）",
    saved: "保存済み",
    saving: "保存中…",
    saveFailed: "保存に失敗しました",
    stats: (chars: number, lines: number) => `${chars} 文字・${lines} 行`,
    updatedAt: (date: string) => `更新: ${date}`,
    tagsPlaceholder: "タグ（カンマ区切り）",
    folderLabel: "フォルダー",
    fmtHeading: "見出し",
    fmtBold: "太字",
    fmtItalic: "斜体",
    fmtStrike: "取り消し線",
    fmtList: "箇条書き",
    fmtCheck: "チェックリスト",
    fmtQuote: "引用",
    fmtCode: "インラインコード",
    fmtCodeBlock: "コードブロック",
    fmtTable: "表",
    fmtLink: "リンク",
    fmtHr: "区切り線",
    // 個人メモ → 共有メモへコピー(M5 F-3、ADR-0049)
    copyToShared: "共有メモへコピー",
    copyToSharedChoose: "コピー先のネットワークを選択",
    copiedToShared: "共有メモへコピーしました",
    // メモ間リンク・バックリンク(M5 F-5 Stage 2、ADR-0052 決定 2)
    wikilinkMissing: "リンク先のメモが見つかりません",
    backlinksTitle: (n: number) => `バックリンク (${n})`,

    // リマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6)。
    // 共有メモ側(SharedMemoView)もこのキーを使う(t.memo.toTrash 等と同じ流儀)
    reminder: "リマインダー",
    reminderTitle: "リマインダーを設定",
    reminderLabel: "日時",
    reminderSave: "設定",
    reminderClear: "解除",
    reminderSaved: "リマインダーを設定しました",
    reminderCleared: "リマインダーを解除しました",
    reminderAt: (date: string) => `リマインダー: ${date}`,
  },

  // 共有ハブ(M5 F-5 Stage 1、ADR-0052 決定 3)。サブタブは現在「メモ」の
  // み。今後スケジュール・表を足す際はここへキーを追加するだけでよい
  sharedHub: {
    tabMemos: "メモ",
  },

  // 共有メモ(M5 F-2、ADR-0049)
  sharedMemo: {
    title: "共有メモ",
    loadFailed:
      "共有メモを読み込めません。デーモンが起動しているか、バージョンが古くないか確認してください",
    offline: "オフライン(ホスト未接続)のため読み取り専用です",
    unsupported:
      "ホストと同期できていません(ホストのバージョンが古い可能性があります)",
    scopeAll: "共有メモ",
    folders: "共有フォルダー",
    empty: "共有メモがありません",
    selectPrompt: "左の一覧から共有メモを選ぶか、新規作成してください",
    viewing: "閲覧中(リアルタイム更新)",
    editingBy: (name: string) => `${name} が編集中`,
    startEdit: "編集",
    stopEdit: "編集を終了",
    forceUnlock: "ロックを強制解除",
    forceUnlockConfirm:
      "編集ロックを強制解除しますか？(編集者に未保存の内容がある可能性があります)",
    perms: "権限の設定",
    permsTitle: "共有メモの権限",
    permsNote:
      "権限はメンバー識別子・グループに紐付きます。優先順位は「メンバー個別 > グループ > 全体」です(「見せない」で特定のメンバー/グループだけ除外できます)。",
    everyoneLabel: "全体(ネットワークの全メンバー)",
    groupsLabel: "グループ",
    groupLevelInherit: "指定なし",
    unknownGroupBadge: "(不明なグループ)",
    levelViewer: "閲覧のみ",
    levelEditor: "閲覧 + 編集",
    levelNone: "見せない",
    levelInherit: "全体に従う",
    viewerBadge: "閲覧のみ",
    copyToPersonal: "個人メモとしてコピー",
    copiedToPersonal: "個人メモへコピーしました(メモタブで開けます)",
    ownerLabel: (name: string) => `所有者: ${name}`,
    hostName: "ホスト",
    updatedBy: (name: string) => `更新者: ${name}`,
    plaintextNote:
      "共有メモはホスト端末へ平文で保存されます。パスワード、秘密鍵、招待トークンなどの保存には使用しないでください。",
    // メモ間リンク・バックリンク(M5 F-5 Stage 2、ADR-0052 決定 2)
    wikilinkMissing: "リンク先のメモが見つかりません",
    backlinksTitle: (n: number) => `バックリンク (${n})`,

    // チャットへのリンクをコピー `@memo:id`(M5 F-5 Stage 4、ADR-0052 決定 1)
    copyLink: "リンクをコピー",
    copyLinkDone: "チャットに貼り付けられるリンクをコピーしました",

    // 変更履歴(M5 F-3、ADR-0049)
    history: "履歴",
    historyEmpty: "変更履歴がありません",
    historySelectPrompt: "左の一覧から版を選んでください",
    historyKind: {
      auto: "自動保存",
      close: "編集終了",
      manual: "手動保存",
      restore: "復元前",
    },
    historyCompare: "現在と比較",
    historyShowBody: "本文を表示",
    historyRestore: "この版へ復元",
    historyRestoreConfirm:
      "この版の内容で現在の本文を置き換えます。現在の内容は履歴に残ります。",
    historyRestored: "この版の内容で復元しました",
    saveVersion: "版を保存",
    saveVersionDone: "版を保存しました",

    // 容量・履歴上限(ホスト管理者のみ、M5 F-3)
    limits: "上限設定",
    limitsTitle: "共有メモの上限",
    limitsNote:
      "共有メモの容量・件数・履歴の保持上限を設定します。範囲外の値は保存時にエラーになります。",
    limitsBodyLabel: "1メモの本文の上限(KiB)",
    limitsTotalLabel: "全体容量の上限(MiB、本文+履歴)",
    limitsCountLabel: "メモ件数の上限(ゴミ箱含む)",
    limitsVersionsLabel: "メモごとの変更履歴の保持件数",
    limitsHistoryDaysLabel: "変更履歴の保持日数",
    limitsTrashDaysLabel: "ゴミ箱の保持日数",
    limitsSaved: "上限を保存しました",

    // コメント・メンション(M5 F-5 Stage 3、ADR-0052 決定 4・5)
    commentsTitle: (n: number) => `コメント (${n})`,
    commentsEmpty: "コメントはまだありません",
    commentPlaceholder: "コメントを入力(@名前 でメンション)",
    commentSend: "送信",
    commentDelete: "削除",
    commentDeleteConfirm: "このコメントを削除しますか?(元に戻せません)",
    commentLoadFailed: "コメントを読み込めません",
    commentTooLong: (kib: number) => `コメントが長すぎます(上限 ${kib}KiB)`,
    commentBadge: (n: number) => `💬${n}`,
  },
};
