// 共有スケジュール表(M6 G-1、ADR-0053)。共有メモの基盤(ホスト正本 DB・
// コントロールチャネル配信・読み取りキャッシュ)を転用する。閲覧・追加は
// 全員、編集・削除は作成者 + ホストだけ(`can_edit` で判定)。編集ロックは
// 持たず revision CAS のみ(ADR-0053 決定 4)。
// **予定のタイトル・詳細は console に出さないこと。**
import { useCallback, useEffect, useMemo, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  Member,
  ScheduleEvent,
  ScheduleOp,
  ScheduleParticipant,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";
import { Modal } from "./Modal";
import { sharedRefToken } from "../sharedRefs";
import { getHolidays, holidayKey } from "../holidays";
import { ScheduleReminderPanel } from "./ReminderButton";

// ---- 日付ユーティリティ(すべてローカル時刻扱い) ----

function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function startOfMonth(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

function addMonths(date: Date, n: number): Date {
  return new Date(date.getFullYear(), date.getMonth() + n, 1);
}

function addDays(date: Date, n: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + n);
  return next;
}

function isSameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function formatTime(unixMs: number): string {
  const d = new Date(unixMs);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function monthLabel(date: Date): string {
  return `${date.getFullYear()}年${date.getMonth() + 1}月`;
}

function dayLabel(date: Date): string {
  return `${date.getMonth() + 1}月${date.getDate()}日(${t.schedule.weekdays[date.getDay()]})`;
}

/** yyyy-mm-dd(date input 用)。 */
function dateInputValue(unixMs: number): string {
  const d = new Date(unixMs);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** yyyy-mm-ddTHH:mm(datetime-local input 用)。 */
function dateTimeInputValue(unixMs: number): string {
  const d = new Date(unixMs);
  return `${dateInputValue(unixMs)}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** "yyyy-mm-dd" のローカル 0:00。 */
function startOfDayMs(dateStr: string): number {
  const [y, m, d] = dateStr.split("-").map(Number);
  return new Date(y, m - 1, d, 0, 0, 0, 0).getTime();
}

/** "yyyy-mm-dd" のローカル 23:59:59.999(終日予定の終了日を含めるため)。 */
function endOfDayMs(dateStr: string): number {
  return startOfDayMs(dateStr) + 24 * 60 * 60 * 1000 - 1;
}

/** 終日 ⇔ 時刻付きの切替時、date / datetime-local の文字列表現を変換する
 * (先頭 10 文字 "yyyy-mm-dd" は共通なので文字列操作だけで済む)。 */
function reformatForAllDay(input: string, allDay: boolean): string {
  if (!input) return input;
  return allDay ? input.slice(0, 10) : `${input.slice(0, 10)}T09:00`;
}

function compareEvents(a: ScheduleEvent, b: ScheduleEvent): number {
  if (Boolean(a.all_day) !== Boolean(b.all_day)) return a.all_day ? -1 : 1;
  return a.start_unix_ms - b.start_unix_ms;
}

/**
 * 参加メンバー選択・「自分の予定」判定に使う id(ADR-0055 決定 5)。
 * ホストは `memberId` が無く(ADR-0047)、スケジュールの `owner_id` 側でも
 * 空文字 = ホストという規約があるため、そちらに揃える。それ以外で
 * `memberId` が無い旧形式メンバーは名前ベースの代替 id にする(実装上の
 * 判断。詳細は作業報告を参照)。
 */
function participantKey(member: Member): string {
  if (member.memberId) return member.memberId;
  if (member.isHost) return "";
  return `name:${member.name ?? member.ip}`;
}

/** 土曜 = 青、日曜・祝日 = 赤(ADR-0055 決定 4)。 */
function dowColorClass(day: Date, holidayName: string | undefined): string {
  if (holidayName) return "schedule__dow--holiday";
  const dow = day.getDay();
  if (dow === 0) return "schedule__dow--sun";
  if (dow === 6) return "schedule__dow--sat";
  return "";
}

interface DialogState {
  mode: "create" | "edit";
  id?: string;
  baseRevision?: number;
  title: string;
  note: string;
  allDay: boolean;
  startInput: string;
  endInput: string;
  participantIds: string[];
}

export function ScheduleView({
  configPath,
  isHost,
  supported,
  seq,
  members,
  focusEventId,
  onFocusConsumed,
}: {
  configPath: string;
  isHost: boolean;
  /** 共有メモ(相乗り)が使える状態か(member で false = ホスト未対応)。 */
  supported: boolean;
  /** 変更世代。進んだら再取得する。 */
  seq: number;
  /** 参加メンバー選択・「自分の予定」判定に使う(ADR-0055 決定 5)。 */
  members: Member[];
  /** チャットの `@schedule:id` カード(ADR-0053)から開く予定。 */
  focusEventId?: string | null;
  onFocusConsumed?: () => void;
}) {
  const [month, setMonth] = useState(() => startOfMonth(new Date()));
  const [selectedDay, setSelectedDay] = useState<Date | null>(() =>
    startOfDay(new Date()),
  );
  const [events, setEvents] = useState<ScheduleEvent[]>([]);
  const [offline, setOffline] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(
    null,
  );
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [saving, setSaving] = useState(false);
  const [holidays, setHolidays] = useState<Record<string, string>>({});
  const [filterMine, setFilterMine] = useState(false);

  useEffect(() => {
    void getHolidays().then(setHolidays);
  }, []);

  const self = useMemo(() => members.find((m) => m.isSelf) ?? null, [members]);
  const selfId = self ? participantKey(self) : null;
  const isMine = useCallback(
    (event: ScheduleEvent) => {
      if (!self || selfId === null) return false;
      if (event.owner_id === selfId) return true;
      if (self.isHost && event.owner_id === "") return true;
      return (event.participants ?? []).some((p) => p.member_id === selfId);
    },
    [self, selfId],
  );

  const scheduleOp = useCallback(
    async (op: ScheduleOp) => {
      const reply = await api.sharedMemoOp(configPath, {
        op: "schedule",
        schedule: op,
      });
      if (reply.kind !== "schedule") {
        throw new Error(`想定外の応答です: ${reply.kind}`);
      }
      return reply.reply;
    },
    [configPath],
  );

  const refresh = useCallback(async () => {
    try {
      const reply = await scheduleOp({ op: "list" });
      if (reply.kind === "events") {
        setEvents(reply.events);
        setOffline(reply.offline ?? false);
        setLoadError(null);
      }
    } catch (error) {
      setLoadError(errorMessage(error));
    } finally {
      setLoaded(true);
    }
  }, [scheduleOp]);

  useEffect(() => {
    void refresh();
    // seq(共有メモの変更世代)が進むたびに再取得 = リアルタイム反映
  }, [refresh, seq]);

  useEffect(() => {
    if (notice === null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  // チャットの `@schedule:id` カードから開く(一覧が届いてから 1 回だけ)
  useEffect(() => {
    if (!focusEventId || !loaded) return;
    const event = events.find((e) => e.id === focusEventId);
    if (event) {
      const day = startOfDay(new Date(event.start_unix_ms));
      setMonth(startOfMonth(day));
      setSelectedDay(day);
      setSelectedEvent(event);
    }
    onFocusConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusEventId, loaded]);

  // 「自分の予定」フィルタ(ADR-0055 決定 5)。カレンダー・日別リストの
  // 両方に効かせる
  const visibleEvents = useMemo(
    () => (filterMine ? events.filter(isMine) : events),
    [events, filterMine, isMine],
  );

  const eventsForDay = useCallback(
    (day: Date) => {
      const dayStart = startOfDay(day).getTime();
      const dayEnd = dayStart + 24 * 60 * 60 * 1000 - 1;
      return visibleEvents
        .filter((event) => {
          const end = event.end_unix_ms ?? event.start_unix_ms;
          return event.start_unix_ms <= dayEnd && end >= dayStart;
        })
        .sort(compareEvents);
    },
    [visibleEvents],
  );

  const gridDays = useMemo(() => {
    const first = startOfMonth(month);
    const gridStart = addDays(first, -first.getDay());
    return Array.from({ length: 42 }, (_, i) => addDays(gridStart, i));
  }, [month]);

  const readOnlyReason = offline
    ? t.schedule.offline
    : !supported && !isHost
      ? t.schedule.unsupported
      : null;

  const openCreateDialog = (day: Date) => {
    setDialog({
      mode: "create",
      title: "",
      note: "",
      allDay: false,
      startInput: dateTimeInputValue(
        new Date(day.getFullYear(), day.getMonth(), day.getDate(), 9, 0).getTime(),
      ),
      endInput: "",
      participantIds: [],
    });
  };

  const openEditDialog = (event: ScheduleEvent) => {
    const allDay = Boolean(event.all_day);
    setDialog({
      mode: "edit",
      id: event.id,
      baseRevision: event.revision,
      title: event.title,
      note: event.note ?? "",
      allDay,
      startInput: allDay
        ? dateInputValue(event.start_unix_ms)
        : dateTimeInputValue(event.start_unix_ms),
      endInput: event.end_unix_ms
        ? allDay
          ? dateInputValue(event.end_unix_ms)
          : dateTimeInputValue(event.end_unix_ms)
        : "",
      participantIds: (event.participants ?? []).map((p) => p.member_id),
    });
  };

  const toggleParticipant = (id: string, checked: boolean) => {
    setDialog((prev) => {
      if (!prev) return prev;
      const next = checked
        ? [...prev.participantIds, id]
        : prev.participantIds.filter((pid) => pid !== id);
      return { ...prev, participantIds: next };
    });
  };

  const toggleAllDay = (allDay: boolean) => {
    setDialog((prev) =>
      prev
        ? {
            ...prev,
            allDay,
            startInput: reformatForAllDay(prev.startInput, allDay),
            endInput: reformatForAllDay(prev.endInput, allDay),
          }
        : prev,
    );
  };

  const onSaveErr = useCallback(
    (message: string) => {
      if (message.includes("competing_edit")) {
        setNotice(t.schedule.conflictNotice);
        setDialog(null);
        void refresh();
      } else {
        setNotice(message);
      }
    },
    [refresh],
  );

  const submitDialog = useCallback(async () => {
    if (!dialog) return;
    const title = dialog.title.trim();
    if (!title || !dialog.startInput) return;
    const start_unix_ms = dialog.allDay
      ? startOfDayMs(dialog.startInput)
      : new Date(dialog.startInput).getTime();
    const end_unix_ms = dialog.endInput
      ? dialog.allDay
        ? endOfDayMs(dialog.endInput)
        : new Date(dialog.endInput).getTime()
      : undefined;
    const participants: ScheduleParticipant[] = dialog.participantIds
      .map((id) => {
        const member = members.find((m) => participantKey(m) === id);
        return member ? { member_id: id, name: member.name ?? member.ip } : null;
      })
      .filter((p): p is ScheduleParticipant => p !== null);
    setSaving(true);
    try {
      const reply =
        dialog.mode === "create"
          ? await scheduleOp({
              op: "create",
              title,
              note: dialog.note.trim(),
              start_unix_ms,
              end_unix_ms,
              all_day: dialog.allDay,
              participants,
            })
          : await scheduleOp({
              op: "update",
              id: dialog.id!,
              base_revision: dialog.baseRevision!,
              title,
              note: dialog.note.trim(),
              start_unix_ms,
              end_unix_ms,
              all_day: dialog.allDay,
              participants,
            });
      if (reply.kind === "event") {
        setDialog(null);
        setSelectedDay(startOfDay(new Date(reply.event.start_unix_ms)));
        void refresh();
      } else if (reply.kind === "err") {
        onSaveErr(reply.message);
      }
    } catch (error) {
      onSaveErr(errorMessage(error));
    } finally {
      setSaving(false);
    }
  }, [dialog, scheduleOp, refresh, onSaveErr, members]);

  const deleteEvent = useCallback(
    async (event: ScheduleEvent) => {
      if (!window.confirm(t.schedule.deleteConfirm)) return;
      try {
        const reply = await scheduleOp({ op: "delete", id: event.id });
        if (reply.kind === "done") {
          setSelectedEvent(null);
          void refresh();
        } else if (reply.kind === "err") {
          setNotice(reply.message);
        }
      } catch (error) {
        setNotice(errorMessage(error));
      }
    },
    [scheduleOp, refresh],
  );

  if (loadError !== null) {
    return (
      <section className="card card--error">
        <h2>{t.schedule.title}</h2>
        <p>{t.schedule.loadFailed}</p>
        <pre className="error-detail">{loadError}</pre>
        <button type="button" onClick={() => void refresh()}>
          {t.common.retry}
        </button>
      </section>
    );
  }

  const selectedDayEvents = selectedDay ? eventsForDay(selectedDay) : [];

  return (
    <div className="schedule">
      <section className="schedule__calendar card">
        <div className="schedule__header">
          <div className="schedule__nav">
            <button
              type="button"
              className="button--icon"
              onClick={() => setMonth((m) => addMonths(m, -1))}
            >
              ◀
            </button>
            <span className="schedule__month">{monthLabel(month)}</span>
            <button
              type="button"
              className="button--icon"
              onClick={() => setMonth((m) => addMonths(m, 1))}
            >
              ▶
            </button>
          </div>
          <div className="schedule__header-right">
            <button
              type="button"
              onClick={() => {
                const today = startOfDay(new Date());
                setMonth(startOfMonth(today));
                setSelectedDay(today);
              }}
            >
              {t.schedule.today}
            </button>
            <div className="schedule__filter" role="group">
              <button
                type="button"
                className={
                  filterMine
                    ? "button--ghost"
                    : "button--ghost schedule__filter-btn--active"
                }
                onClick={() => setFilterMine(false)}
              >
                {t.schedule.filterAll}
              </button>
              <button
                type="button"
                className={
                  filterMine
                    ? "button--ghost schedule__filter-btn--active"
                    : "button--ghost"
                }
                onClick={() => setFilterMine(true)}
              >
                {t.schedule.filterMine}
              </button>
            </div>
          </div>
        </div>
        {readOnlyReason && (
          <p className="schedule__notice small">{readOnlyReason}</p>
        )}
        {notice && <p className="schedule__notice small">{notice}</p>}
        <div className="schedule__weekdays">
          {t.schedule.weekdays.map((w, i) => (
            <span
              key={i}
              className={i === 0 ? "schedule__dow--sun" : i === 6 ? "schedule__dow--sat" : undefined}
            >
              {w}
            </span>
          ))}
        </div>
        <div className="schedule__grid">
          {gridDays.map((day, i) => {
            const inMonth = day.getMonth() === month.getMonth();
            const today = isSameDay(day, new Date());
            const active = selectedDay !== null && isSameDay(day, selectedDay);
            const dayEvents = eventsForDay(day);
            const holidayName = holidays[holidayKey(day)];
            return (
              <button
                type="button"
                key={i}
                className={[
                  "schedule__cell",
                  !inMonth && "schedule__cell--muted",
                  today && "schedule__cell--today",
                  active && "schedule__cell--active",
                ]
                  .filter(Boolean)
                  .join(" ")}
                onClick={() => setSelectedDay(day)}
              >
                <span className="schedule__cell-date-row">
                  <span
                    className={
                      "schedule__cell-date " + dowColorClass(day, holidayName)
                    }
                  >
                    {day.getDate()}
                  </span>
                  {holidayName && (
                    <span
                      className="schedule__holiday-badge"
                      title={t.schedule.holidayBadgeTitle(holidayName)}
                    >
                      {holidayName}
                    </span>
                  )}
                </span>
                <span className="schedule__cell-events">
                  {dayEvents.slice(0, 3).map((event) => (
                    <span key={event.id} className="schedule__event-chip">
                      {event.all_day
                        ? event.title
                        : `${formatTime(event.start_unix_ms)} ${event.title}`}
                      {(event.participants?.length ?? 0) > 0 &&
                        ` ${t.schedule.participantsBadgeCount(event.participants!.length)}`}
                    </span>
                  ))}
                  {dayEvents.length > 3 && (
                    <span className="schedule__more">
                      {t.schedule.more(dayEvents.length - 3)}
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      </section>

      <aside className="schedule__day card">
        <div className="schedule__day-head">
          <span className="schedule__day-title">
            {selectedDay ? dayLabel(selectedDay) : ""}
            {selectedDay && holidays[holidayKey(selectedDay)] && (
              <span className="schedule__holiday-badge schedule__holiday-badge--inline">
                {holidays[holidayKey(selectedDay)]}
              </span>
            )}
          </span>
          <button
            type="button"
            disabled={readOnlyReason !== null || !selectedDay}
            onClick={() => selectedDay && openCreateDialog(selectedDay)}
          >
            ＋ {t.schedule.addEvent}
          </button>
        </div>
        {!selectedDay && (
          <p className="muted small schedule__placeholder">
            {t.schedule.selectDayPrompt}
          </p>
        )}
        {selectedDay && (
          <ul className="schedule__day-list">
            {selectedDayEvents.length === 0 && (
              <li className="muted small schedule__day-empty">
                {t.schedule.noEventsForDay}
              </li>
            )}
            {selectedDayEvents.map((event) => (
              <li key={event.id}>
                <button
                  type="button"
                  className="schedule__day-item"
                  onClick={() => setSelectedEvent(event)}
                >
                  <span className="schedule__day-item-time muted small">
                    {event.all_day
                      ? t.schedule.allDayLabel
                      : formatTime(event.start_unix_ms)}
                  </span>
                  <span className="schedule__day-item-title">
                    {event.title}
                  </span>
                  {(event.participants?.length ?? 0) > 0 && (
                    <span className="muted small">
                      {t.schedule.participantsBadgeCount(
                        event.participants!.length,
                      )}
                    </span>
                  )}
                  {!event.can_edit && (
                    <span className="tag">{t.schedule.viewerBadge}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </aside>

      {dialog && (
        <Modal
          title={
            dialog.mode === "create"
              ? t.schedule.createTitle
              : t.schedule.editTitle
          }
          onClose={() => setDialog(null)}
        >
          <div className="field">
            <label>{t.schedule.titleLabel}</label>
            <input
              autoFocus
              value={dialog.title}
              placeholder={t.schedule.titlePlaceholder}
              onChange={(event) =>
                setDialog({ ...dialog, title: event.target.value })
              }
            />
          </div>
          <label className="schedule__allday-label">
            <input
              type="checkbox"
              checked={dialog.allDay}
              onChange={(event) => toggleAllDay(event.target.checked)}
            />
            {t.schedule.allDayLabel}
          </label>
          <div className="field">
            <label>{t.schedule.startLabel}</label>
            <input
              type={dialog.allDay ? "date" : "datetime-local"}
              value={dialog.startInput}
              onChange={(event) =>
                setDialog({ ...dialog, startInput: event.target.value })
              }
            />
          </div>
          <div className="field">
            <label>{t.schedule.endLabel}</label>
            <input
              type={dialog.allDay ? "date" : "datetime-local"}
              value={dialog.endInput}
              onChange={(event) =>
                setDialog({ ...dialog, endInput: event.target.value })
              }
            />
          </div>
          <div className="field">
            <label>{t.schedule.noteLabel}</label>
            <textarea
              rows={4}
              value={dialog.note}
              placeholder={t.schedule.notePlaceholder}
              onChange={(event) =>
                setDialog({ ...dialog, note: event.target.value })
              }
            />
          </div>
          <div className="field">
            <label>{t.schedule.participantsLabel}</label>
            <div className="schedule__participants-picker">
              {members.map((member) => {
                const id = participantKey(member);
                const checked = dialog.participantIds.includes(id);
                return (
                  <label key={id} className="schedule__participant-option">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(event) =>
                        toggleParticipant(id, event.target.checked)
                      }
                    />
                    {member.name ?? member.ip}
                    {member.isSelf && (
                      <span className="muted small">
                        {" "}
                        {t.schedule.selfBadge}
                      </span>
                    )}
                  </label>
                );
              })}
            </div>
          </div>
          <div className="modal__actions">
            <button
              type="button"
              className="button--ghost"
              onClick={() => setDialog(null)}
            >
              {t.common.cancel}
            </button>
            <button
              type="button"
              disabled={!dialog.title.trim() || !dialog.startInput || saving}
              onClick={() => void submitDialog()}
            >
              {saving ? t.common.running : t.common.save}
            </button>
          </div>
        </Modal>
      )}

      {selectedEvent && (
        <Modal
          title={t.schedule.detailTitle}
          onClose={() => setSelectedEvent(null)}
        >
          <h3 className="schedule__detail-title">{selectedEvent.title}</h3>
          <p className="muted small">
            {selectedEvent.all_day
              ? dayLabel(new Date(selectedEvent.start_unix_ms)) +
                ` ${t.schedule.allDayLabel}` +
                (selectedEvent.end_unix_ms &&
                !isSameDay(
                  new Date(selectedEvent.start_unix_ms),
                  new Date(selectedEvent.end_unix_ms),
                )
                  ? ` 〜 ${dayLabel(new Date(selectedEvent.end_unix_ms))}`
                  : "")
              : `${dayLabel(new Date(selectedEvent.start_unix_ms))} ${formatTime(selectedEvent.start_unix_ms)}` +
                (selectedEvent.end_unix_ms
                  ? ` 〜 ${formatTime(selectedEvent.end_unix_ms)}`
                  : "")}
          </p>
          {(selectedEvent.note ?? "").trim() !== "" && (
            <p className="schedule__detail-note">{selectedEvent.note}</p>
          )}
          {(selectedEvent.participants?.length ?? 0) > 0 && (
            <div className="schedule__participant-badges">
              {selectedEvent.participants!.map((p) => (
                <span key={p.member_id} className="tag">
                  {p.name}
                </span>
              ))}
            </div>
          )}
          <div className="schedule__detail-meta">
            <span className="muted small">
              {t.schedule.ownerLabel(
                selectedEvent.owner_name || t.sharedMemo.hostName,
              )}
            </span>
            {selectedEvent.updated_by && (
              <span className="muted small">
                {t.schedule.updatedByLabel(selectedEvent.updated_by)}
              </span>
            )}
          </div>
          <ScheduleReminderPanel
            network={configPath}
            eventId={selectedEvent.id}
            startUnixMs={selectedEvent.start_unix_ms}
            onNotice={setNotice}
          />
          <div className="modal__actions">
            <button
              type="button"
              className="button--icon"
              title={t.schedule.copyLink}
              onClick={() =>
                void writeText(
                  sharedRefToken("schedule", selectedEvent.id),
                ).then(() => setNotice(t.schedule.copyLinkDone))
              }
            >
              🔗
            </button>
            {selectedEvent.can_edit && readOnlyReason === null && (
              <>
                <button
                  type="button"
                  onClick={() => {
                    openEditDialog(selectedEvent);
                    setSelectedEvent(null);
                  }}
                >
                  ✏ {t.schedule.editEvent}
                </button>
                <button
                  type="button"
                  className="button--ghost button--ghost-danger"
                  onClick={() => void deleteEvent(selectedEvent)}
                >
                  🗑 {t.schedule.deleteEvent}
                </button>
              </>
            )}
          </div>
        </Modal>
      )}
    </div>
  );
}
