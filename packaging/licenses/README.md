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
