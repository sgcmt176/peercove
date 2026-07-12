# M3-14c 実装指示書(Opus 向け): サービス情報と URL コピー

- **作成**: 2026-07-12、Claude Fable 5(設計担当)
- **実装担当**: Claude Opus
- **前提コミット**: main の最新(M3-14b = ADR-0022 と検証フィードバック修正を
  含む)。**着手前に必ず `git pull`**。M3-14b は実機検証中のため、検証で
  さらに修正が入る可能性がある — 着手時に main が進んでいたら差分を読むこと
- **必読**: `CLAUDE.md` → `docs/roadmap.md`(§5 振り分けガイド・開発
  ワークフロー)→ `docs/decisions.md` の ADR-0011 / 0021 / 0022

## 1. ゴール(依頼者要望 13 + 11 の残り)

カスタム DNS レコード(ADR-0022)に**サービス情報(スキーム・ポート)**を
持たせ、UI で **URL をワンクリックでコピー**できるようにする。

例: エイリアス `gamehost`(→ 山田さんの PC)に `scheme = "http"`,
`port = 8080` を付けると、UI の一覧に
`http://gamehost.home.peercove.internal:8080/` が出て「URL をコピー」できる。
メンバー側の DNS 画面でも同じ URL が見える(閲覧のみ)。

**やらないこと**(このゴールの対象外。実装しない):

- ヘルスチェック・オンライン連動(14e)、逆引き(14d)、ワイルドカード(14f)、
  短縮名(14g)
- SRV レコード等の DNS プロトコル拡張(DNS 応答は A レコードのみのまま)
- CLI へのレコード操作コマンド追加(レコード管理は UI のみ — ADR-0022 §7)

## 2. 設計(確定済み — 変更する場合は Fable に差し戻すこと)

### 2.1 設定(host.toml)

`DnsRecordConfig`(`crates/peercove-core/src/config.rs`)に追加:

```toml
[[dns_record]]
name = "gamehost"
member = "<公開鍵>"
scheme = "http"   # 任意。URI スキーム(小文字英数と + . -、先頭は英字、31 文字以内)
port = 8080       # 任意。1〜65535
```

- 両方 `Option` + `#[serde(skip_serializing_if = "Option::is_none")]`
- `Config::validate` に形式検証を追加(不正なら明示エラー)。
  正規表現は使わず文字種チェックで書く(既存の names.rs の流儀)
- 既存フィールドと同じく **`deny_unknown_fields` のため、これを書いた設定は
  旧バージョンでは読めない**(doc コメントに明記 — dns_name 等と同じ形式)

### 2.2 ワイヤ(台帳配布)

`crates/peercove-core/src/dns.rs` の `DnsRecord`(配布形式)に追加:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub scheme: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub port: Option<u16>,
```

- **PROTO_VERSION / IPC_VERSION は上げない**。`DnsRecord` は
  `deny_unknown_fields` ではないので、旧メンバーは未知フィールドを無視して
  名前解決は従来どおり動く(URL 表示だけ新バージョン限定)
- `None` ならワイヤに現れない = 旧バージョンとワイヤ表現が一致する。
  `crates/peercove-core/src/proto.rs` に既存の互換テストがあるので、
  同じ流儀で「scheme/port なしのレコードは旧形式と同一 JSON」を確認する
  テストを足すこと
- `resolve_records`(dns.rs)は scheme/port をそのまま通す(コピー)。
  既存テスト `resolve_records_follows_member_ip_and_label` を拡張してよい
- `zone_for` と DNS サーバ(`crates/peercove-poc/src/dns.rs`)は**触らない**
  (A レコード応答に scheme/port は無関係)

### 2.3 ops(`crates/peercove-ops/src/dns.rs`)

- `add_record` の引数が増えるので、`NewPeer`(peers.rs)に倣って
  **構造体にまとめ直す**:

  ```rust
  pub struct NewRecord<'a> {
      pub name: &'a str,
      pub target: RecordTarget,
      pub under: Option<MemberRef>,
      pub scheme: Option<&'a str>,
      pub port: Option<u16>,
  }
  pub fn add_record(config_path: &Path, record: &NewRecord) -> anyhow::Result<String>
  ```

  呼び出し側は `apps/peercove-ui/src-tauri/src/lib.rs` の `add_dns_record` と
  ops 内テスト(dns.rs / peers.rs の `records_follow_rotation_and_removal`)のみ
- scheme は書き込む前に小文字化 + 形式検証(validate と同じ規則。
  エラーメッセージは人間向けに)
- `RecordDetail` に `scheme: Option<String>`, `port: Option<u16>`,
  `url: Option<String>` を追加。`url` は組み立て済み文字列:
  - scheme あり: `{scheme}://{fqdn}{:port?}/`(port が無ければ省略。
    http=80 / https=443 の既定ポートも省略する)
  - scheme なし + port あり: `url = None`(UI は `fqdn:port` を出す)
  - どちらも無し: `None`(従来表示)

### 2.4 デーモン・DTO

- `TunnelInfo.dns_records`(status 経由の配布済みレコード)は wire の
  `DnsRecord` なので scheme/port が自動で載る(daemon.rs の変更は不要のはず)
- `apps/peercove-ui/src-tauri/src/dto.rs`:
  - `DnsRecordDto` に `scheme` / `port` / `url` を追加
  - `From<RecordDetail>`(ホストの一覧)と `Tunnel::from` 内のマッピング
    (メンバーの一覧)の両方に反映。メンバー側の `url` 組み立ては
    RecordDetail と同じ規則の小さなヘルパー関数を dto.rs に置く
    (規則が 2 か所に割れないよう、可能なら core か ops の関数を共用する)

### 2.5 UI(`apps/peercove-ui/src`)

- `ipc.ts`: `DnsRecord` 型と `api.addDnsRecord` に scheme/port を追加
- `DnsDialog.tsx` の追加フォーム(ホストのみ)に任意入力を追加:
  - 「スキーム」: `<input list>`(datalist で http / https を候補に、
    自由入力可、空 = なし)
  - 「ポート」: number input(空 = なし)
- 一覧(ホスト・メンバー共通):
  - `url` があれば URL を表示し、既存の「コピー」に加えて
    「URL をコピー」ボタンを出す(既存 `copyButton` の流儀)
  - `url` が無く port だけあれば `fqdn:port` 表記
- 文言はすべて `i18n/ja.tsx` の `dns` セクションへ(直書き禁止)

### 2.6 ドキュメント

- `docs/decisions.md` に **ADR-0023** を追記(背景 / 選択肢 / 決定 / 理由 /
  対象外)。本指示書 §2 の内容を ADR の体裁に清書する。選択肢として
  「SRV レコード配信」を挙げ、「A レコード + メタ情報の配布(採用)」との
  比較(旧バージョン互換・OS リゾルバの SRV 非対応)を理由に書くこと
- `docs/roadmap.md`: M3-14 行の 14c を「実装済み(ADR-0023)、実機検証待ち」へ
- `README.md`: 「カスタム DNS レコードの拡張(M3-14b)」の直後に
  「サービス情報と URL コピー(M3-14c)」セクション + **検証手順**(5 項目
  程度: 追加 → 全員に URL が見える → コピーしてブラウザで開ける →
  旧レコードの表示が変わらない → 不正 scheme/port の拒否)

## 3. 作業ルール(CLAUDE.md の要点 + このリポジトリの実務)

- **cargo は PATH 前置が必要**:
  `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`(PowerShell 5.1。
  `&&` は使えない)
- **ゲート(すべて通してからコミット)**:
  1. `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
     `cargo test --workspace`(リポジトリルート)
  2. `cargo-zigbuild clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
  3. `apps/peercove-ui/src-tauri` で `cargo fmt` / `clippy -D warnings` /
     `cargo test`
  4. `apps/peercove-ui` で `npm run build`(tsc + vite)
- **秘密鍵・PSK・トークンをログ・標準出力に出さない**(公開鍵は可)
- コミットは 1 ゴール 1 コミット。メッセージは日本語で
  「M3-14c: サービス情報と URL コピー(ADR-0023)」+ 本文に要点。
  末尾に自分の Co-Authored-By トレーラーを付ける
- push 後に GitHub Actions(4 ジョブ: rust/ui × windows/ubuntu)の成功を
  確認してから、依頼者に**実機検証を依頼して停止**する(自動テストで
  ネットワーク実疎通の検証はしない — README の手順に委ねる)

## 4. 完了条件チェックリスト

- [ ] config: scheme/port の解析・検証 + テスト
- [ ] wire: DnsRecord 拡張 + 互換テスト(None = 旧形式と同一 JSON)
- [ ] resolve_records が scheme/port を通す + テスト
- [ ] ops: NewRecord 化 + RecordDetail.url + テスト(既定ポート省略を含む)
- [ ] dto/lib.rs: ホスト・メンバー両経路の DTO に反映 + dto テスト更新
- [ ] UI: フォーム(スキーム・ポート)+ 一覧の URL 表示 + URL コピー
- [ ] i18n: 文言追加
- [ ] ADR-0023 / roadmap / README(検証手順つき)
- [ ] 全ゲート + CI 成功 → 実機検証依頼で停止
