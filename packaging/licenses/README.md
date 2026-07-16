# 第三者ライセンス(インストーラ同梱用)

このフォルダには、インストーラ(MSI / deb / ZIP)へ**そのまま同梱する**第三者
ライセンス本文を置きます。再配布素材なのでリポジトリには含めず(gitignore 済み)、
**インストーラをビルドする前に手で配置**します。

## wintun-LICENSE.txt(必須)

PeerCove は wintun.net の署名済み `wintun.dll` を無改変で同梱します
(M2 handoff Q5・ADR-0010 で確認済み。Permitted API を wintun crate 経由でのみ
使用するため Prebuilt Binaries License の範囲内)。**同じ ZIP に入っている
ライセンス本文を同梱する義務**があります。

配置手順(`wintun.dll` の入手と対で行う):

```
# wintun.net からダウンロードした wintun-<version>.zip を展開し、
#   zip 直下の LICENSE.txt を           → packaging/licenses/wintun-LICENSE.txt
#   zip 内 bin/amd64/wintun.dll を        → apps/peercove-ui/src-tauri/windows/wintun.dll
# へコピーする。両方とも同じ zip の版を使うこと(版がずれないように)。
```

DLL とライセンスは**必ず同じ zip の版**から取ること。ライセンス本文は版に
よって変わりうるため、`wintun.dll` と `wintun-LICENSE.txt` の版を一致させます。

インストーラは、ここに置かれた `wintun-LICENSE.txt` をインストール先の
`licenses\wintun-LICENSE.txt`(Windows)/ `/usr/share/doc/peercove/`(deb)へ
配置します。アプリの謝辞にも wintun への言及を含めます。

## 第三者 crate / npm パッケージの謝辞

PeerCove 本体は MIT OR Apache-2.0(デュアル)ですが、依存 crate・npm
パッケージにはそれぞれのライセンス表示義務があります。依存ライセンスの監査は
[development.md](../../docs/development.md) の「依存ライセンスの確認」を参照。

- 現状の依存に **GPL / AGPL などの強いコピーレフトは無し**。単独のコピーレフトは
  **MPL-2.0(弱い・ファイル単位)** のみ(Rust 側: attohttpc, uniffi ほか /
  UI 側: cssparser, selectors ほか)。いずれも無改変で利用しており、デュアル
  ライセンスでの配布と両立します。MPL-2.0 は該当ファイルのソース入手性を保つ
  義務があるだけで、成果物全体のライセンスには影響しません。
- **配布時の対応**: リリースの `.msi` / `.deb` / ZIP には、依存(Rust + npm)の
  ライセンス表示を集約した `THIRD-PARTY-NOTICES.txt` を同梱します。生成は
  `packaging/make-notices.sh`(Rust は `cargo about` = `about.toml` / `about.hbs`、
  npm は `packaging/collect-npm-licenses.mjs` が `node_modules` を走査)。
  ポータブル ZIP / tar は生成物があれば自動同梱します。MSI / deb への埋め込みは
  現状手動(生成した `THIRD-PARTY-NOTICES.txt` をインストール先の `licenses\` /
  `/usr/share/doc/peercove/` へ含める)。手順は
  [development.md](../../docs/development.md) の「第三者ライセンス謝辞」を参照。
