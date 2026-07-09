# PeerCove ポータブル版(上級者向け)

インストーラを使わない配布形態です。**インストーラ(MSI / deb)を使えるなら
そちらを推奨します**(サービス登録・自動起動・アンインストール・通知の表示名まで
面倒を見ます)。このポータブル版は検証や上級者向けで、セットアップは手動です。

## 同梱物

- `peercove-poc(.exe)` … CLI + デーモン本体
- `PeerCove`(UI 実行ファイル)… デスクトップ画面
- `wintun.dll`(Windows のみ)… TUN ドライバ(wintun.net の署名済みバイナリ、無改変)
- `wintun-LICENSE.txt`(Windows のみ)… 上記の再配布ライセンス

## 使い方

トンネル操作には管理者 / root 権限のデーモンが必要です。2 通りあります。

### A. サービスとして常駐させる(再起動後も自動起動)

```
# Windows: 管理者 PowerShell
.\peercove-poc.exe daemon service-install      # 登録 + 起動 + ファイアウォール許可

# Linux: root
sudo ./peercove-poc daemon service-install
```

その後 `PeerCove`(UI)を**通常権限で**起動します。やめるときは:

```
# Windows(管理者) / Linux(root)
peercove-poc daemon service-uninstall          # 停止 + 登録解除 + 残骸の片付け
```

### B. その場だけ動かす(常駐しない)

```
# Windows(管理者) / Linux(sudo)。このターミナルを閉じるまで動く
peercove-poc daemon run
```

別ターミナル(通常権限)で `PeerCove`(UI)を起動します。終了は Ctrl+C。

> **サービスと `daemon run` は同時に動かせません**(パイプ/ソケットが衝突します)。

## 注意点

- **Windows のポータブル版では、通知の表示元が「PowerShell」になります。**
  これはインストール(スタートメニューのショートカット)で解消される
  Windows の仕様です。MSI 版では「PeerCove」と出ます。
- Windows は `peercove-poc.exe` と同じフォルダに `wintun.dll` が必要です
  (同梱済み)。別の場所へ exe だけ移すとトンネルを作れません。
- Linux でトレイアイコンを出すには `libayatana-appindicator3` が必要です
  (`sudo apt install libayatana-appindicator3-1`)。
- 設定ファイル・鍵の既定の置き場所は、Windows `%APPDATA%\app.peercove.desktop\`、
  Linux `~/.config/app.peercove.desktop/` です(UI が表示します)。
