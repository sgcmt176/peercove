// 予定のリマインダー(端末ローカル、複数件可、ADR-0055 決定 3)。
//
// 個人メモ・共有メモの ⏰ リマインダー UI(旧 ReminderButton、単発設定)は
// MemoView.tsx / SharedMemoView.tsx から撤去され、この新コンポーネントへ
// 置き換わった(ScheduleView.tsx の予定詳細から使う)。保存先の DB・
// 発火処理(notify.ts の周期ポーリング + OS 通知)は同じ仕組みを流用する。
// scope は "schedule" 固定、network = 共有スケジュールの configPath、
// memo_id = 予定 id。1 予定につき複数件、上限 10 件(サーバー側で強制)。
import { useCallback, useEffect, useState } from "react";
import { MemoReminder, api, errorMessage } from "../ipc";
import { t } from "../i18n";

const MAX_REMINDERS = 10;
const PRESET_MINUTES = [5, 15, 30, 60, 1440] as const;

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function formatDateTime(unixMs: number): string {
  const d = new Date(unixMs);
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function formatTime(unixMs: number): string {
  const d = new Date(unixMs);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function presetLabel(minutes: number): string {
  switch (minutes) {
    case 5:
      return t.schedule.reminderPreset5;
    case 15:
      return t.schedule.reminderPreset15;
    case 30:
      return t.schedule.reminderPreset30;
    case 60:
      return t.schedule.reminderPreset60;
    case 1440:
      return t.schedule.reminderPreset1440;
    default:
      return t.schedule.reminderPresetGeneric(minutes);
  }
}

/** `datetime-local` の初期値(1 時間後)。 */
function defaultCustomValue(): string {
  const d = new Date(Date.now() + 60 * 60 * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function ScheduleReminderPanel({
  network,
  eventId,
  startUnixMs,
  onNotice,
}: {
  /** 共有スケジュールの configPath。 */
  network: string;
  eventId: string;
  startUnixMs: number;
  onNotice: (message: string) => void;
}) {
  const [reminders, setReminders] = useState<MemoReminder[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [customValue, setCustomValue] = useState(defaultCustomValue);

  // ReminderList(全件)から、この予定宛の分だけ絞り込む(専用の絞り込み
  // IPC は無いため — H-3a の memo_op はこの形。開始時刻昇順)。
  const refresh = useCallback(async () => {
    try {
      const reply = await api.memoOp({ op: "reminder_list" });
      if (reply.kind === "reminders") {
        setReminders(
          reply.reminders
            .filter(
              (r) =>
                r.scope === "schedule" &&
                (r.network ?? "") === network &&
                r.memo_id === eventId,
            )
            .sort((a, b) => a.remind_at - b.remind_at),
        );
      }
    } catch (error) {
      onNotice(errorMessage(error));
    } finally {
      setLoaded(true);
    }
  }, [network, eventId, onNotice]);

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [network, eventId]);

  const add = useCallback(
    async (remindAt: number, offsetMinutes?: number) => {
      setBusy(true);
      try {
        await api.memoOp({
          op: "reminder_set",
          scope: "schedule",
          network,
          memo_id: eventId,
          remind_at: remindAt,
          offset_minutes: offsetMinutes,
        });
        await refresh();
      } catch (error) {
        onNotice(errorMessage(error));
      } finally {
        setBusy(false);
      }
    },
    [network, eventId, refresh, onNotice],
  );

  const remove = useCallback(
    async (remindAt: number) => {
      setBusy(true);
      try {
        await api.memoOp({
          op: "reminder_clear",
          scope: "schedule",
          network,
          memo_id: eventId,
          remind_at: remindAt,
        });
        await refresh();
      } catch (error) {
        onNotice(errorMessage(error));
      } finally {
        setBusy(false);
      }
    },
    [network, eventId, refresh, onNotice],
  );

  const atLimit = reminders.length >= MAX_REMINDERS;
  const now = Date.now();

  const addCustom = () => {
    if (!customValue) return;
    const ms = new Date(customValue).getTime();
    if (Number.isNaN(ms)) return;
    void add(ms);
  };

  return (
    <div className="schedule__reminders">
      <h4 className="schedule__reminders-title">{t.schedule.remindersTitle}</h4>
      {loaded && reminders.length === 0 && (
        <p className="muted small">{t.schedule.reminderEmpty}</p>
      )}
      {reminders.length > 0 && (
        <ul className="schedule__reminder-list">
          {reminders.map((reminder) => (
            <li key={reminder.remind_at} className="schedule__reminder-item">
              <span>
                {reminder.offset_minutes != null
                  ? t.schedule.reminderOffsetLabel(
                      presetLabel(reminder.offset_minutes),
                      formatTime(reminder.remind_at),
                    )
                  : formatDateTime(reminder.remind_at)}
              </span>
              <button
                type="button"
                className="button--icon"
                disabled={busy}
                title={t.common.delete}
                onClick={() => void remove(reminder.remind_at)}
              >
                🗑
              </button>
            </li>
          ))}
        </ul>
      )}
      {atLimit && (
        <p className="muted small">{t.schedule.reminderLimitReached}</p>
      )}
      <div className="schedule__reminder-presets">
        {PRESET_MINUTES.map((minutes) => {
          const remindAt = startUnixMs - minutes * 60_000;
          const disabled = busy || atLimit || remindAt <= now;
          return (
            <button
              key={minutes}
              type="button"
              className="button--ghost"
              disabled={disabled}
              onClick={() => void add(remindAt, minutes)}
            >
              {presetLabel(minutes)}
            </button>
          );
        })}
      </div>
      <div className="schedule__reminder-custom">
        <input
          type="datetime-local"
          value={customValue}
          disabled={busy || atLimit}
          onChange={(event) => setCustomValue(event.target.value)}
        />
        <button
          type="button"
          className="button--ghost"
          disabled={busy || atLimit || !customValue}
          onClick={addCustom}
        >
          {t.schedule.reminderAdd}
        </button>
      </div>
      <p className="muted small">{t.schedule.reminderHelp}</p>
    </div>
  );
}
