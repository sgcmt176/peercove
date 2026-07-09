# G7b MSI 方式変更の確認依頼(→ Fable / 依頼者レビュー用)

- **作成**: 2026-07-09(Opus、G7b 実装中)
- **状態**: 依頼者は「CLI 呼び出し方式で実装」を選択済み。**後日 Fable に妥当性を確認する**
- **目的**: ADR-0010 決定 5(MSI は WiX ネイティブの `<ServiceInstall>` で登録)を
  変更したので、その理由と実装を Fable が短時間でレビューできるようにまとめる

## 1. 何を変えたか(1 行)

MSI のサービス登録を **WiX ネイティブ `<ServiceInstall>/<ServiceControl>`(宣言的)**
から、**カスタムアクションで `peercove-poc.exe daemon service-install` /
`service-uninstall` を呼ぶ(手続き的)**方式へ変更した。

## 2. なぜ変えたか(新情報 3 点)

ADR-0010 決定 5 は「MSI は WiX ServiceInstall で登録」としていた。しかし G7a の
実機検証と G7b の Tauri 調査で、当時は分かっていなかった事実が 3 つ出た:

1. **Session 0 のサービスにはファイアウォール許可が必須**(ADR-0010「Session 0 の
   追加要件」)。UDP(WG 待受)と TCP(コントロールチャネル)の**両方**。
   これが無いと「ping は通るが台帳が来ない」等になる。**MSI の ServiceInstall は
   ファイアウォールを一切面倒見ない**ので、別機構が要る。
2. **Tauri 2.11 の WiX 設定には任意の WiX 拡張(`WixFirewallExtension`)を渡す口が
   無い**(`bundle.windows.wix` は fragmentPaths / componentRefs / template /
   banner / language 等のみ。`-ext` 相当が無い)。よって「WiX の宣言的機能で
   ファイアウォールを入れる」案は**使えない** → どのみちカスタムアクションが要る。
3. `service.rs` の `daemon service-install` は **サービス登録 + UDP/TCP
   ファイアウォールを一括で行い、2026-07-09 に実機で動作確認済み**
   (`77b7b6e` / `98cfae4`。SYSTEM ACL・UDP・TCP の 3 つの Session 0 ブロッカーを
   すべて潰した状態)。

**結論**: WiX ネイティブ方式でも**ファイアウォール用のカスタムアクションは必須**。
それなら、サービス登録も含めて「実機検証済みの CLI」を 1 回呼ぶ方が、
(a) 二重定義が無く(b) 検証済みコードの再利用になる。

## 3. どちらの案でもカスタムアクションは必要(比較表)

| 観点 | 採用案: CLI 呼び出し | 却下案: WiX ネイティブ + 別 CA |
|---|---|---|
| サービス登録 | CA が `service-install` を呼ぶ | `<ServiceInstall>`(宣言的) |
| ファイアウォール | 上記 CLI に内包(検証済み) | **別途** netsh の CA を UDP/TCP で 2 本 |
| カスタムアクション数 | 3(install / uninstall / rollback) | 3〜4(fw add ×1〜2 / fw delete / 順序制御) |
| サービス定義の在処 | `service.rs` の定数のみ(単一) | `service.rs` **と** `.wxs` に二重 |
| ルール名の在処 | `service.rs` の `FIREWALL_RULE` のみ | `service.rs` と `.wxs` に二重(ずれると残骸) |
| 検証済みか | **実機で丸ごと検証済み** | ServiceInstall 部分は新規・未検証 |
| 失敗時ロールバック | rollback CA を自前で用意 | ServiceInstall は自動 / fw CA は自前 |

**唯一の実質的トレードオフ**はロールバック。WiX ネイティブは ServiceInstall の
自動ロールバックがあるが、ファイアウォール CA 部分は結局自前ロールバックが要る。
採用案では install CA の直前に rollback CA(`service-uninstall`)を置いて対処する。

## 4. 実装の要点(レビューで見てほしい所)

- カスタムアクションは **type 18(`FileKey` 参照)**。インストールされる
  `peercove-poc.exe`(`File Id="PeercoveDaemonExe"`)を FileKey にするので、
  `[INSTALLDIR]` の遅延プロパティ展開・CustomActionData が不要
  (deferred CA の定番の落とし穴を回避)。WiX 拡張も不要。
- 実行タイミング:
  - `PeercoveServiceInstall`: `Execute="deferred" Impersonate="no"`(= 昇格/SYSTEM
    コンテキスト)、`After="InstallFiles"`(exe と wintun.dll が配置済みの後)、
    条件 `NOT Installed`、`Return="check"`(失敗で MSI 全体をロールバック)
  - `PeercoveServiceInstallRollback`: `Execute="rollback"`、install CA の直前に配置、
    `service-uninstall` を `Return="ignore"` で
  - `PeercoveServiceUninstall`: `Execute="deferred" Impersonate="no"`、
    `Before="RemoveFiles"`(exe がまだ在る間に)、条件
    `Installed AND REMOVE~="ALL"`、`Return="ignore"`
- `service.rs` は**一切変更していない**(呼ぶだけ)。サービス名 `peercove-daemon`・
  引数 `daemon service`・ルール名 `PeerCove Daemon` はすべて `service.rs` の定数が
  単一の正本。ADR-0010 の「変えてはいけないもの」を維持。

## 5. Fable に確認したい点(3 つ)

1. **`daemon service-install` を MSI の deferred CA(SYSTEM)から呼んで安全か。**
   `service.start()` は StartService を投げるだけで RUNNING を待たない実装なので
   MSI を長時間ブロックしない、という理解で合っているか。
2. **`service-uninstall` の停止待ち(最大 30 秒)を MSI のアンインストール中に
   走らせて問題ないか。** タイムアウトで bail するが、CA は `Return="ignore"` に
   しているのでアンインストール自体は続行する設計でよいか。
3. **将来 MSI 由来のサービスを CLI `service-uninstall` で消しても(逆も)整合するか。**
   両経路で同じ `service.rs` を通るので大丈夫という理解でよいか
   (MSI が二重に消そうとしても、既に無ければ「登録されていません」で無害終了)。

## 6. 変更しなかったもの(念のため)

`service.rs` / `daemon.rs` / `control.rs` / `backend/` は無変更。WiX フラグメントの
サービス定義部分(ServiceInstall/ServiceControl)は CA へ置き換えたが、
File 要素(exe / wintun.dll の配置)は残している。
