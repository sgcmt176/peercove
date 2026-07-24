// メモのリマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6)。
// 個人メモ(MemoView)・共有メモ(SharedMemoView)の両方から使う共通部品
// (保存先はどちらも個人メモ DB。t.memo.reminder* のキーも共通で使う —
// t.memo.toTrash 等を SharedMemoView が使い回すのと同じ流儀)。
//
// ADR-0055 決定 3: メモの ⏰ リマインダー UI は MemoView.tsx /
// SharedMemoView.tsx から撤去し、スケジュールの予定リマインダー(実装順
// H-3)へ移設する。この部品自体(ダイアログ・IPC 呼び出し)と notify.ts
// の発火処理・ストアは H-3 で流用するため、あえて未使用のまま残してある。
// 消さないこと。
import { useState } from "react";
import { MemoReminder, ReminderScope, api, errorMessage } from "../ipc";
import { t } from "../i18n";
import { Modal } from "./Modal";

export function ReminderButton({
  scope,
  network,
  memoId,
  reminder,
  onChanged,
  onNotice,
}: {
  scope: ReminderScope;
  /** 共有メモのときだけ渡す(configPath)。個人メモは省略。 */
  network?: string;
  memoId: string;
  reminder: MemoReminder | null;
  onChanged: (reminder: MemoReminder | null) => void;
  onNotice: (message: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);

  const openDialog = () => {
    setValue(toLocalInputValue(reminder?.remind_at));
    setOpen(true);
  };

  const save = async () => {
    const ms = new Date(value).getTime();
    if (Number.isNaN(ms)) return;
    setBusy(true);
    try {
      await api.memoOp({
        op: "reminder_set",
        scope,
        network,
        memo_id: memoId,
        remind_at: ms,
      });
      onChanged({ scope, network, memo_id: memoId, remind_at: ms });
      onNotice(t.memo.reminderSaved);
      setOpen(false);
    } catch (error) {
      onNotice(errorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setBusy(true);
    try {
      await api.memoOp({ op: "reminder_clear", scope, network, memo_id: memoId });
      onChanged(null);
      onNotice(t.memo.reminderCleared);
      setOpen(false);
    } catch (error) {
      onNotice(errorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        type="button"
        className={
          reminder ? "button--icon button--icon--active" : "button--icon"
        }
        title={
          reminder ? t.memo.reminderAt(formatDateTime(reminder.remind_at)) : t.memo.reminder
        }
        onClick={openDialog}
      >
        ⏰
      </button>
      {open && (
        <Modal title={t.memo.reminderTitle} onClose={() => setOpen(false)}>
          <div className="modal__body">
            <label className="memo__reminder-label">
              {t.memo.reminderLabel}
              <input
                type="datetime-local"
                value={value}
                onChange={(event) => setValue(event.target.value)}
              />
            </label>
          </div>
          <div className="modal__actions">
            {reminder && (
              <button
                type="button"
                className="button--ghost button--ghost-danger"
                disabled={busy}
                onClick={() => void clear()}
              >
                {t.memo.reminderClear}
              </button>
            )}
            <button type="button" className="button--ghost" onClick={() => setOpen(false)}>
              {t.common.cancel}
            </button>
            <button type="button" disabled={busy || !value} onClick={() => void save()}>
              {t.memo.reminderSave}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

/** `remind_at`(UNIX ミリ秒)→ `datetime-local` の値(ローカル時刻)。
 * 未設定なら 1 時間後を初期値にする(すぐ入力できるように)。 */
function toLocalInputValue(remindAt?: number): string {
  const date = remindAt ? new Date(remindAt) : new Date(Date.now() + 60 * 60 * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function formatDateTime(unixMs: number): string {
  const date = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
