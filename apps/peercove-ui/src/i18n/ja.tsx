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
    settings: "設定",
    settingsUnavailable:
      "設定ファイルがまだありません（ホストを始めるか参加してください）",
    logs: "デーモンのログ",
  },

  footer: (
    <>
      wintun.dll © WireGuard LLC — Prebuilt Binaries License の下で無改変同梱
      （インストール先の <span className="mono">wintun-LICENSE.txt</span>{" "}
      を参照）。
    </>
  ),

  daemonOutdated: {
    title: "デーモンの更新が必要です",
    body: "動いているデーモンが古いバージョンのため、状態を正しく表示できません（トンネルが稼働中でも「停止中」に見えます）。サービスを入れ替えてください:",
    windows:
      "管理者 PowerShell: Stop-Service peercove-daemon → 新しい peercove-poc.exe で daemon service-uninstall → daemon service-install",
    linux:
      "sudo systemctl stop peercove-daemon → 新しいバイナリを配置 → sudo systemctl start peercove-daemon",
  },

  daemonUnreachable: {
    title: "デーモンに接続できません",
    body: "トンネルの操作には管理者権限のデーモンが必要です。ターミナルで次を実行してください:",
    command: "peercove-poc daemon run",
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
    disconnectConfirm: "切断する",
    disconnect: "切断",
    configFileLabel: "設定ファイル",
    membersHead: (n: number) => `メンバー（${n}）`,
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
      rx: "受信",
      tx: "送信",
      notConnected: "(未接続)",
      rttTitle: "トンネル内のコントロールチャネルで測った往復時間",
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
      rename: "名前を変更",
      remove: "削除",
      self: "自分",
      selfTitle: "このマシン自身です",
      // メンバー間の経路(M3-4、ADR-0013)
      route: {
        direct: "直接",
        trying: "確立中…",
        relay: "中継",
        title:
          "このメンバーとの通信経路。「直接」は相手と直接接続中、「中継」はホスト経由、「確立中…」は直接接続を試しているところです。",
      },
    },
    // 直接通信の説明(M3-4)。外部 IP の共有について明記する(ADR-0013 条件 3)
    directNote:
      "メンバー同士は可能なら直接通信します(速く・ホスト回線の負荷なし)。このとき、あなたのグローバル IP アドレスが同じネットワークのメンバーに共有されます。使いたくない場合は「設定」で直接通信をオフにしてください。",
  },

  // DNS 管理画面(M3-1c)
  dns: {
    title: "DNS",
    button: "DNS",
    intro: (
      <>
        メンバーは <code>名前.ネットワーク名.peercove.internal</code>{" "}
        で呼べます。名前はメンバー一覧の表示名から自動で決まります。
      </>
    ),
    autoHead: "自動レコード（メンバー）",
    autoEmpty: "台帳をまだ受信していません。",
    customHead: "カスタムレコード",
    customEmpty: "カスタムレコードはありません。",
    customNote:
      "ホストだけが追加・削除できます。追加すると約 10 秒で全メンバーに配布されます。",
    customNoteMember: "カスタムレコードはホストが管理します（ここでは閲覧のみ）。",
    nameLabel: "名前（ラベル）",
    namePlaceholder: "nas",
    ipLabel: "IPv4 アドレス",
    ipPlaceholder: "10.68.1.50",
    add: "追加",
    adding: "追加中…",
    remove: "削除",
    copy: "コピー",
    copied: "コピーしました",
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
    yes: "あり",
    no: "なし",
    copied: "コピーしました",
    copy: "トークンをコピー",
    resultNote:
      "同じ LAN のメンバーは LAN 側の候補、別ネットワークのメンバーは外部の候補で接続します。取り消すときはメンバー一覧から削除してください。",
    formTitle: "メンバーを招待",
    nameLabel: "名前（省略すると自動で付きます）",
    namePlaceholder: "alice",
    externalLabel: "外部の接続先（別ネットワークの人を招く場合）",
    externalHint: "LAN 内の接続先は自動で入ります。省略しても構いません。",
    externalPlaceholder: "203.0.113.5:51820",
    pskLabel: "事前共有鍵（PSK）も発行する",
    issuing: "発行中…",
    issue: "招待を発行",
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

  settings: {
    title: "設定",
    mtuInteger: "MTU には整数を入力してください",
    portInteger: "待受ポートには整数を入力してください",
    savedRestart: "保存しました。切断して接続し直すと反映されます。",
    savedLive: "保存しました。数秒でトンネルに反映されます。",
    interface: "インターフェース",
    displayNameLabel: "表示名（メンバー一覧に出る名前）",
    displayNamePlaceholderMember: "（未設定）",
    displayNamePlaceholderHost: "host",
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
    restartHint: (isMember: boolean) =>
      `待受ポート・MTU${
        isMember ? "・ホストのエンドポイント" : ""
      }は、トンネルを作り直すまで反映されません（切断 → 接続で反映されます）。`,
  },

  // OS 通知(notify.ts)
  notify: {
    joinedTitle: "メンバーが参加しました",
    leftTitle: "メンバーが切断しました",
    body: (name: string, ip: string, network: string) =>
      `${name}（${ip}）— ${network}`,
  },
};
