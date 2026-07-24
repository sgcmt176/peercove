//! 共有スケジュール表の共有型(ADR-0053、M6 G-1)。
//!
//! 共有メモ([`crate::memo`])の基盤(ホスト正本 DB・コントロールチャネル配信・
//! 読み取りキャッシュ)を転用する。ストレージ実装は `peercove-memo` crate。
//! **予定のタイトル・詳細はチャット本文と同格の秘匿対象 — ログ・標準出力へ
//! 出さない**(id・件数は可)。
//!
//! [`crate::memo::SharedMemoOp`] / `SharedMemoReply` / `SharedMemoEvent` へ
//! additive な `Schedule` variant として相乗りする(capability は
//! `shared_memo` のまま。スケジュール未対応の旧クライアントは該当行を
//! 解析失敗で無視する = 既存の互換モデル、ADR-0053 決定 5)。

use serde::{Deserialize, Serialize};

/// タイトルの上限(文字数)。
pub const MAX_SCHEDULE_TITLE_CHARS: usize = 200;

/// 詳細(note)の上限(バイト、UTF-8)。
pub const MAX_SCHEDULE_NOTE_BYTES: usize = 4 * 1024;

/// 1 ネットワークあたりの予定件数上限。
pub const MAX_SCHEDULE_EVENTS: u32 = 10_000;

/// 共有スケジュール表の予定 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleEvent {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub start_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_unix_ms: Option<u64>,
    /// 終日予定は日付単位で扱う(ADR-0053 決定 2)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_day: bool,
    /// 所有者(作成者)の member_id。空文字 = ホスト。
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub owner_name: String,
    /// 最終更新者の表示名(スナップショット)。
    #[serde(default)]
    pub updated_by: String,
    /// 単調増加リビジョン(CAS 用、ADR-0053 決定 4)。
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// 受信者視点: 編集・削除できるか(作成者 + ホスト、ADR-0053 決定 3)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_edit: bool,
}

/// 共有スケジュール表への操作。ネットワーク全員が閲覧・追加でき、
/// 編集・削除は作成者 + ホストだけができる(ADR-0053 決定 3)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ScheduleOp {
    /// 全件一覧(V1。件数上限 [`MAX_SCHEDULE_EVENTS`] のため全量で良い)。
    List,
    /// 新規作成(全員可)。応答は作成した予定の [`ScheduleReply::Event`]。
    Create {
        #[serde(default)]
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
        start_unix_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_unix_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_day: bool,
    },
    /// 更新(CAS)。作成者・ホストのみ。`base_revision` が現在と一致しない
    /// 場合は競合として拒否される。応答は更新後の [`ScheduleReply::Event`]。
    Update {
        id: String,
        base_revision: u64,
        #[serde(default)]
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        note: String,
        start_unix_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_unix_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_day: bool,
    },
    /// 削除(物理削除、ゴミ箱なし = V1)。作成者・ホストのみ。
    Delete { id: String },
}

/// [`ScheduleOp`] への応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleReply {
    /// List への応答。
    Events {
        events: Vec<ScheduleEvent>,
        /// (メンバーのみ)ホスト未接続のキャッシュ応答 = 読み取り専用。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    /// Create / Update への応答。
    Event { event: ScheduleEvent },
    /// Delete への応答。
    Done,
    /// 拒否・競合など(コントロールチャネル経由の応答用)。
    Err { message: String },
}

/// ホスト → メンバーのリアルタイム配信。**閲覧権限のフィルタなし**
/// (全員閲覧、ADR-0053 決定 3)。受信者ごとの `can_edit` は配信時に
/// 計算し直して詰める。tag は `kind`(`event` は [`crate::memo::SharedMemoEvent`]
/// 側のタグと衝突するため使えない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleEventMsg {
    /// 作成・更新。
    Changed { event: ScheduleEvent },
    /// 削除。キャッシュから消すこと。
    Removed { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ワイヤ表現(UI・モバイルが依存)。追加フィールドはすべて省略可能で、
    /// 旧バージョンとの相互無視が成り立つことを固定する。
    #[test]
    fn schedule_op_wire_format() {
        let op = ScheduleOp::List;
        assert_eq!(serde_json::to_string(&op).unwrap(), r#"{"op":"list"}"#);
        assert_eq!(
            serde_json::from_str::<ScheduleOp>(r#"{"op":"list"}"#).unwrap(),
            op
        );

        let op = ScheduleOp::Create {
            title: "定例会議".to_string(),
            note: String::new(),
            start_unix_ms: 1_000,
            end_unix_ms: None,
            all_day: false,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"op":"create","title":"定例会議","start_unix_ms":1000}"#
        );
        assert_eq!(serde_json::from_str::<ScheduleOp>(&json).unwrap(), op);

        let op = ScheduleOp::Update {
            id: "e1".to_string(),
            base_revision: 3,
            title: "改題".to_string(),
            note: "詳細".to_string(),
            start_unix_ms: 2_000,
            end_unix_ms: Some(3_000),
            all_day: true,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<ScheduleOp>(&json).unwrap(), op);

        let op = ScheduleOp::Delete {
            id: "e1".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            r#"{"op":"delete","id":"e1"}"#
        );
        assert_eq!(
            serde_json::from_str::<ScheduleOp>(&serde_json::to_string(&op).unwrap()).unwrap(),
            op
        );

        let event = ScheduleEvent {
            id: "e1".to_string(),
            title: "t".to_string(),
            note: String::new(),
            start_unix_ms: 1,
            end_unix_ms: None,
            all_day: false,
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            updated_by: "ホスト".to_string(),
            revision: 1,
            created_at: 1,
            updated_at: 1,
            can_edit: true,
        };
        let reply = ScheduleReply::Event {
            event: event.clone(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<ScheduleReply>(&json).unwrap(), reply);

        let msg = ScheduleEventMsg::Changed { event };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            serde_json::from_str::<ScheduleEventMsg>(&json).unwrap(),
            msg
        );

        let msg = ScheduleEventMsg::Removed {
            id: "e1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"kind":"removed","id":"e1"}"#);
        assert_eq!(
            serde_json::from_str::<ScheduleEventMsg>(&json).unwrap(),
            msg
        );
    }
}
