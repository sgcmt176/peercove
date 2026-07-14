//! デーモンが保持する直近ログのリングバッファ(M2-G5、ADR-0009)。
//!
//! UI はデーモンの標準エラー出力を読めない(別プロセス・別権限で動くため)。
//! そこで `tracing` の Layer としてログ行をメモリに溜め、IPC の
//! [`peercove_core::ipc::IpcRequest::Logs`] で取り出せるようにする。
//!
//! ファイルへ書き出さないのは、
//! - root/管理者で作られたファイルの権限を UI 側で扱う必要が出る
//! - ローテーション・削除漏れ(「残骸を残さない」方針)を新たに抱える
//!
//! ため。デーモンが死ねばログも消えるが、常駐前提なので実用上の困りは小さい。
//!
//! 秘密鍵・PSK・トークンはそもそもログへ出さない方針(CLAUDE.md)なので、
//! ここに溜まった行はそのまま UI へ渡してよい。

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use peercove_core::ipc::{LogLine, MAX_LOG_LINES_PER_REPLY};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// 保持する行数。1 行 ~150 バイトとして数十 KiB 程度。
const CAPACITY: usize = 500;

pub struct LogRing {
    inner: Mutex<Inner>,
}

struct Inner {
    lines: VecDeque<LogLine>,
    next_seq: u64,
}

/// プロセスに 1 つ。`tracing` の Layer と IPC ハンドラの双方から触る。
pub fn ring() -> &'static LogRing {
    static RING: OnceLock<LogRing> = OnceLock::new();
    RING.get_or_init(|| LogRing {
        inner: Mutex::new(Inner {
            lines: VecDeque::with_capacity(CAPACITY),
            next_seq: 1,
        }),
    })
}

impl LogRing {
    pub(crate) fn push(&self, level: &str, target: &str, message: String) {
        // ロックの中で panic すると以後のログが全部失われるため、何もしない
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.lines.len() == CAPACITY {
            inner.lines.pop_front();
        }
        inner.lines.push_back(LogLine {
            seq,
            unix_ms: unix_ms(),
            level: level.to_string(),
            target: target.to_string(),
            message,
        });
    }

    /// `after_seq` より後の行を最大 [`MAX_LOG_LINES_PER_REPLY`] 件返す。
    ///
    /// 2 つ目の戻り値は、`after_seq` の続きのはずが溢れて失われていた行数。
    /// 初回取得(`after_seq == 0`)は「持っている分すべて」の意味なので 0 とする。
    pub fn since(&self, after_seq: u64) -> (Vec<LogLine>, u64) {
        let Ok(inner) = self.inner.lock() else {
            return (Vec::new(), 0);
        };
        let oldest = inner.lines.front().map(|line| line.seq).unwrap_or(0);
        let dropped = if after_seq == 0 || oldest == 0 {
            0
        } else {
            oldest.saturating_sub(after_seq + 1)
        };
        let lines: Vec<LogLine> = inner
            .lines
            .iter()
            .filter(|line| line.seq > after_seq)
            .take(MAX_LOG_LINES_PER_REPLY)
            .cloned()
            .collect();
        (lines, dropped)
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ログ行をリングバッファへ複製する Layer。整形は fmt レイヤと独立。
pub struct RingLayer;

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        ring().push(
            metadata.level().as_str(),
            metadata.target(),
            visitor.finish(),
        );
    }
}

/// `message` フィールドを本文に、その他のフィールドを `key=value` として後ろへ。
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn finish(self) -> String {
        format!("{}{}", self.message, self.fields)
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // message は `format_args!` なので `{:?}` で引用符なしに出る
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> LogRing {
        LogRing {
            inner: Mutex::new(Inner {
                lines: VecDeque::new(),
                next_seq: 1,
            }),
        }
    }

    #[test]
    fn since_returns_only_newer_lines() {
        let ring = fresh();
        for i in 0..3 {
            ring.push("INFO", "t", format!("line {i}"));
        }
        let (lines, dropped) = ring.since(0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].seq, 1);
        assert_eq!(dropped, 0);

        let (lines, dropped) = ring.since(2);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].message, "line 2");
        assert_eq!(dropped, 0);

        assert_eq!(ring.since(3).0.len(), 0);
    }

    #[test]
    fn overflow_evicts_oldest_and_reports_dropped() {
        let ring = fresh();
        for i in 0..(CAPACITY + 10) {
            ring.push("INFO", "t", format!("line {i}"));
        }
        let (lines, dropped) = ring.since(0);
        assert_eq!(lines.len(), MAX_LOG_LINES_PER_REPLY);
        assert_eq!(dropped, 0, "初回取得では溢れを報告しない");

        // seq 1..=10 は溢れている。seq 5 の続きを求めると 5 行分が失われている
        let (lines, dropped) = ring.since(5);
        assert_eq!(lines[0].seq, 11);
        assert_eq!(dropped, 5);

        // 追いつけている(溢れていない)ときは 0
        assert_eq!(ring.since(400).1, 0);
    }

    /// 特定のマーカーを含む行を探す。`ring()` はプロセス全体で 1 つなので、
    /// 並行して走る他テストの行と混ざらないよう本文で絞る。
    fn find(marker: &str) -> Option<LogLine> {
        ring()
            .since(0)
            .0
            .into_iter()
            .find(|line| line.message.contains(marker))
    }

    /// 実際の `tracing` イベントが、本文 + `key=value` の 1 行になること。
    /// (購読者はこのスレッドだけに入れる)
    #[test]
    fn layer_captures_message_and_fields() {
        use tracing_subscriber::prelude::*;

        let subscriber = tracing_subscriber::registry().with(RingLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(peer = "alice", "capture-marker トンネル {} を作成", 1);
        });

        let line = find("capture-marker").expect("イベントが記録される");
        assert_eq!(line.level, "WARN");
        assert_eq!(
            line.message,
            r#"capture-marker トンネル 1 を作成 peer="alice""#
        );
        assert!(line.target.starts_with("peercove"));
    }

    /// フィルタは fmt レイヤとリングバッファで共通(main.rs の `init_tracing` と同じ構成)。
    /// `--log-level warn` にすると、ログビューにも info 以下は出ない。
    #[test]
    fn layer_honors_the_global_filter() {
        use tracing_subscriber::prelude::*;

        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("warn"))
            .with(RingLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("filtered-marker 出ないはず");
            tracing::warn!("kept-marker 出るはず");
        });

        assert!(find("filtered-marker").is_none(), "info は捨てられる");
        assert!(find("kept-marker").is_some(), "warn は残る");
    }
}
