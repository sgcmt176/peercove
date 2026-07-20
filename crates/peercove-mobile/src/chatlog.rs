//! チャット履歴のローカル保存(ADR-0016)。デスクトップ(peercove-cli/chat.rs)の
//! ChatLog の移植(IPC ページング定数への依存を外した簡略版)。
//!
//! `networks/<slug>/member.toml` → `member.chat.jsonl` に 1 行 1 通で追記する。
//! 履歴は端末ローカルのみ。上限([`MAX_HISTORY`])を超えたら古い方から捨てる。
//!
//! 秘匿ルール: 本文はログへ出さない(seq・id・IP は可)。

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use peercove_core::ipc::ChatMessageInfo;

/// 保持する履歴の上限(通)。デスクトップと同値。
const MAX_HISTORY: usize = 10_000;
/// ファイルの行数がこの値を超えたら詰め直す(追記のたびに書き直さない)。
const REWRITE_THRESHOLD: usize = MAX_HISTORY + 1_000;

/// 1 ネットワーク分のチャット履歴(メモリ上の直近 [`MAX_HISTORY`] 通 + 追記ファイル)。
pub struct ChatLog {
    path: PathBuf,
    entries: VecDeque<ChatMessageInfo>,
    file_lines: usize,
    next_seq: u64,
}

impl ChatLog {
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_extension("chat.jsonl")
    }

    /// 履歴を読み込む(ファイルが無ければ空。壊れた行は読み飛ばす)。
    pub fn load(config_path: &Path) -> ChatLog {
        let path = Self::path_for(config_path);
        let mut entries: VecDeque<ChatMessageInfo> = VecDeque::new();
        let mut file_lines = 0usize;
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                file_lines += 1;
                if let Ok(entry) = serde_json::from_str::<ChatMessageInfo>(line) {
                    entries.push_back(entry);
                }
            }
        }
        while entries.len() > MAX_HISTORY {
            entries.pop_front();
        }
        let next_seq = entries.back().map(|e| e.seq + 1).unwrap_or(1);
        Self {
            path,
            entries,
            file_lines,
            next_seq,
        }
    }

    /// 1 通追記する(seq はここで振る)。ファイルへの追記に失敗しても
    /// メモリ上の履歴には残す(警告ログのみ)。
    pub fn append(&mut self, mut entry: ChatMessageInfo) -> ChatMessageInfo {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(entry.clone());
        if self.entries.len() > MAX_HISTORY {
            self.entries.pop_front();
        }
        if let Err(e) = self.write_entry(&entry) {
            tracing::warn!("チャット履歴の書き出しに失敗しました: {e:#}");
        }
        entry
    }

    fn write_entry(&mut self, entry: &ChatMessageInfo) -> anyhow::Result<()> {
        if self.file_lines >= REWRITE_THRESHOLD {
            return self.rewrite();
        }
        // 失敗フラグは揮発(ADR-0016)なのでファイルには書かない
        let mut persisted = entry.clone();
        persisted.failed = false;
        let mut line = serde_json::to_string(&persisted).context("履歴の直列化に失敗しました")?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("{} を開けません", self.path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("{} へ追記できません", self.path.display()))?;
        self.file_lines += 1;
        Ok(())
    }

    /// メモリ上の履歴(= 直近 [`MAX_HISTORY`] 通)でファイルを詰め直す。
    fn rewrite(&mut self) -> anyhow::Result<()> {
        let mut buf = String::new();
        for entry in &self.entries {
            let mut persisted = entry.clone();
            persisted.failed = false;
            buf.push_str(&serde_json::to_string(&persisted).context("履歴の直列化に失敗しました")?);
            buf.push('\n');
        }
        let tmp = self.path.with_extension("chat.jsonl.tmp");
        std::fs::write(&tmp, buf).with_context(|| format!("{} へ書けません", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("{} の置き換えに失敗しました", self.path.display()))?;
        self.file_lines = self.entries.len();
        Ok(())
    }

    /// 送信失敗の印を付ける(メモリ上のみ — 再起動で消える)。
    pub fn mark_failed(&mut self, seq: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.seq == seq) {
            entry.failed = true;
        }
    }

    /// 再送が成功したとき失敗の印を外す(E-E 3)。
    pub fn clear_failed(&mut self, seq: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.seq == seq) {
            entry.failed = false;
        }
    }

    /// メッセージ ID が既に履歴にあるか(再送の二重受信防止)。
    pub fn contains_id(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// seq からエントリを引く(手動再送用)。
    pub fn get(&self, seq: u64) -> Option<ChatMessageInfo> {
        self.entries.iter().find(|e| e.seq == seq).cloned()
    }

    /// `after_seq` より後のエントリを最大 `limit` 通返す。
    pub fn fetch(&self, after_seq: u64, limit: usize) -> Vec<ChatMessageInfo> {
        self.entries
            .iter()
            .filter(|e| e.seq > after_seq)
            .take(limit)
            .cloned()
            .collect()
    }

    /// 履歴全体の最新 seq(0 = 履歴なし)。
    pub fn latest_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// 履歴を全消去する(E-E 10 のストレージ管理)。seq は続きから振る
    /// (ポーリングのカーソルや既読位置を壊さない)。呼び出し側は直後に
    /// お知らせ行を 1 行 append して、再起動後も seq が巻き戻らないようにする。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.file_lines = 0;
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::msg::ChatScope;

    fn temp_config(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-mobile-chat-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("member.toml")
    }

    fn entry(text: &str) -> ChatMessageInfo {
        ChatMessageInfo {
            seq: 0,
            id: "c".to_string(),
            scope: ChatScope::Direct,
            group_id: None,
            from: "10.0.0.1".parse().unwrap(),
            to: Some("10.0.0.2".parse().unwrap()),
            text: text.to_string(),
            sent_at: 1_700_000_000_000,
            failed: false,
            file: None,
            system: false,
        }
    }

    #[test]
    fn append_reload_and_fetch() {
        let config = temp_config("roundtrip");
        {
            let mut log = ChatLog::load(&config);
            assert_eq!(log.append(entry("一通目")).seq, 1);
            assert_eq!(log.append(entry("二通目")).seq, 2);
        }
        let log = ChatLog::load(&config);
        assert_eq!(log.latest_seq(), 2);
        let messages = log.fetch(0, 100);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "一通目");
        assert_eq!(log.fetch(1, 100).len(), 1, "after_seq より後だけ");
        assert_eq!(log.fetch(0, 1).len(), 1, "limit で打ち切り");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn mark_failed_is_volatile() {
        let config = temp_config("failed");
        {
            let mut log = ChatLog::load(&config);
            let appended = log.append(entry("届かない"));
            log.mark_failed(appended.seq);
            assert!(log.fetch(0, 10)[0].failed);
        }
        let log = ChatLog::load(&config);
        assert!(!log.fetch(0, 10)[0].failed, "失敗フラグは揮発");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }
}
