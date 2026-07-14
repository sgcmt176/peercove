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
    members: "メンバー",
    chat: "チャット",
    stats: "統計",
    inbox: "受信",
    dns: "DNS",
    subnets: "サブネット",
    settings: "設定",
    connected: "接続中",
    disconnected: "未接続",
    theme: (theme: "light" | "dark") =>
      theme === "light"
        ? "外観: ライト（クリックでダーク）"
        : "外観: ダーク（クリックでライト）",
    logs: "デーモンのログ",
    version: (v: string) => `v${v}`,
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
    note: "この設定はこのマシンにだけ保存されます（即時反映）。",
  },

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
      stats: "統計",
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
      rename: "名前を変更",
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
        title:
          "このメンバーとの通信経路。「直接」は相手と直接接続中、「中継」はホスト経由、「確立中…」は直接接続を試しているところです。",
      },
      // ACL(M3-10、ADR-0018)
      blocked: "通信不可",
      blockedTitle:
        "ホストの通信制御により、このメンバーとの通信は遮断されています",
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
    previewSelf: "自分: ",
    placeholder: "メッセージを入力（Enter で送信、Shift+Enter で改行）",
    offline: "オフラインのメンバーには送れません",
    blocked: "ホストの通信制御により、このメンバーには送れません",
    left: "このメンバーは現在ネットワークにいません（履歴のみ）",
    send: "送信",
    failed: "送信失敗",
    // グループ(M3-13c)
    groupCreate: "グループ作成",
    groupTitle: "新しいグループ",
    groupManage: "管理",
    groupNameLabel: "グループ名",
    groupNamePlaceholder: "例: 開発チーム",
    groupMembersLabel: "メンバーを選ぶ",
    groupAddLabel: "メンバーを追加",
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
    forwardMember: (name: string) => `マシン: ${name}`,
    forwardLocked:
      "ドメインの「この左側」にマシンを選んだので、転送先はそのマシンに固定されます。",
    ipPlaceholder: "10.68.1.50",
    schemeLabel: "スキーム（任意）",
    schemePlaceholder: "http",
    portLabel: "ポート（任意）",
    portPlaceholder: "8080",
    serviceHint:
      "スキームを指定すると URL を組み立てて全メンバーに表示します。ポートだけ指定した場合は DNS名:ポートで表示します。",
    targetOf: (name: string) => `→ ${name}`,
    brokenRef: "参照先なし",
    add: "追加",
    adding: "追加中…",
    remove: "削除",
    copy: "コピー",
    copyUrl: "URL をコピー",
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
    // 招待ディープリンク(M3-5)
    copyLink: "参加リンクをコピー",
    copyLinkHint:
      "参加リンク(peercove://…)は、PeerCove がインストール済みの相手ならクリックだけで参加フォームが開きます。未インストールの相手にはトークンを渡してください。",
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
    displayNameLabel: "表示名（メンバー一覧に出る名前。日本語・空白可）",
    displayNamePlaceholderMember: "（未設定）",
    displayNamePlaceholderHost: "host",
    // DNS 名の分離(ADR-0021、M3-14a)
    dnsNameLabel: "DNS 名",
    dnsNameHint:
      " — <DNS 名>.<ネットワーク名>.peercove.internal の先頭部分。英数字とハイフンのみ（空欄なら host）",
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
    // 受信ファイルサイズ上限(ADR-0015、M3-9)
    maxFileLabel: "受信ファイルの上限（MB）",
    maxFileHint: (defaultMb: number) =>
      ` — 既定 ${defaultMb}。0 で無制限。これを超えるファイルの受信は拒否されます（約 10 秒で反映）`,
    maxFileInteger: "受信ファイルの上限には 0 以上の整数を入力してください",
    restartHint: (isMember: boolean) =>
      `待受ポート・MTU${
        isMember ? "・ホストのエンドポイント" : ""
      }は、トンネルを作り直すまで反映されません（切断 → 接続で反映されます）。`,
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
  },
};
