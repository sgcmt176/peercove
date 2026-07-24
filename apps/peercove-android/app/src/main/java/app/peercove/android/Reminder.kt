package app.peercove.android

// メモのリマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6)。
//
// VPN 接続の有無に関係なく発火させたいので AlarmManager を使う(inexact =
// setAndAllowWhileIdle。SCHEDULE_EXACT_ALARM 権限は要求しない)。発火時は
// memoReminderTakeDue を呼んで fired へ遷移させ、返ってきた分だけタイトルを
// 解決して通知する(個人メモは memoGet、共有メモはキャッシュの
// sharedMemoGet — どちらもホスト接続は不要)。解決できなければ(削除済み・
// アクセス不可)黙って通知しない。
//
// 端末再起動後は BOOT_COMPLETED で memoReminderList から未発火分を
// 再登録する(AlarmManager の登録は再起動で消えるため)。
//
// ADR-0055 決定 3: メモ側の ⏰ 設定メニュー・アイコン(MemoScreen.kt /
// SharedMemoScreen.kt からの呼び出し)は撤去した。この基盤(発火処理・
// pickReminderDateTime/applyReminder/clearReminder/fetchReminders)自体は
// あえて残してある。スケジュールの予定リマインダー(実装順 H-3)で流用する
// ため。既に設定済みのメモリマインダーは引き続き発火する(害はない)。
//
// H-3a(Rust 層)で 1 対象に複数件のリマインダーを許可した(offset_minutes
// 付き)。ただし ReminderScheduler の AlarmManager PendingIntent は今も
// (scope, network, memoId) だけを requestCode にしているため、同じ対象へ
// 複数の remindAtMs を schedule() すると後勝ちで上書きされる。UI 実装
// (H-3 次工程)で複数件を有効化する際は、requestCode に remindAtMs も
// 含めるよう ReminderScheduler を拡張すること。

import android.app.AlarmManager
import android.app.DatePickerDialog
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.TimePickerDialog
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.Calendar
import uniffi.peercove_mobile.MemoReminderInfo
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.ReminderScopeArg
import uniffi.peercove_mobile.memoGet
import uniffi.peercove_mobile.memoReminderClear
import uniffi.peercove_mobile.memoReminderList
import uniffi.peercove_mobile.memoReminderSet
import uniffi.peercove_mobile.memoReminderTakeDue
import uniffi.peercove_mobile.memoRemindersFor
import uniffi.peercove_mobile.sharedMemoGet

/** `ReminderScopeArg` ⇔ ワイヤ表現("personal"/"shared"/"schedule")。
 * Intent extras・AlarmManager の requestCode には文字列の方が扱いやすい
 * ための変換。`SCHEDULE` はスケジュールの予定リマインダー(ADR-0055 決定 3)。 */
fun ReminderScopeArg.wire(): String = when (this) {
    ReminderScopeArg.PERSONAL -> "personal"
    ReminderScopeArg.SHARED -> "shared"
    ReminderScopeArg.SCHEDULE -> "schedule"
}

object ReminderNotifier {
    const val CHANNEL_ID = "peercove_reminder"
    const val EXTRA_SCOPE = "reminder_scope"
    const val EXTRA_NETWORK = "reminder_network"
    const val EXTRA_MEMO_ID = "reminder_memo_id"

    fun ensureChannel(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(
            CHANNEL_ID,
            context.getString(R.string.notif_reminder_channel),
            NotificationManager.IMPORTANCE_HIGH,
        )
        manager.createNotificationChannel(channel)
    }

    fun notificationId(scope: String, network: String, memoId: String): Int =
        "$scope:$network:$memoId".hashCode()

    /** タイトルはローカル通知への表示であり、ログではない(ADR-0049)。 */
    fun show(context: Context, reminder: MemoReminderInfo, title: String) {
        ensureChannel(context)
        val scopeStr = reminder.scope.wire()
        val id = notificationId(scopeStr, reminder.network, reminder.memoId)
        val open = PendingIntent.getActivity(
            context,
            id,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_SCOPE, scopeStr)
                .putExtra(EXTRA_NETWORK, reminder.network)
                .putExtra(EXTRA_MEMO_ID, reminder.memoId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(context.getString(R.string.notif_reminder_title, title))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(id, notification)
    }
}

object ReminderScheduler {
    private const val ACTION_FIRE = "app.peercove.android.action.MEMO_REMINDER"

    private fun pendingIntent(
        context: Context,
        scope: ReminderScopeArg,
        network: String,
        memoId: String,
    ): PendingIntent {
        val scopeStr = scope.wire()
        val requestCode = ReminderNotifier.notificationId(scopeStr, network, memoId)
        val intent = Intent(context, ReminderReceiver::class.java)
            .setAction(ACTION_FIRE)
            .putExtra(ReminderNotifier.EXTRA_SCOPE, scopeStr)
            .putExtra(ReminderNotifier.EXTRA_NETWORK, network)
            .putExtra(ReminderNotifier.EXTRA_MEMO_ID, memoId)
        return PendingIntent.getBroadcast(
            context,
            requestCode,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    /** 登録(inexact)。過去時刻の拒否はストア側(memoReminderSet)が担う。 */
    fun schedule(context: Context, scope: ReminderScopeArg, network: String, memoId: String, remindAtMs: Long) {
        ReminderNotifier.ensureChannel(context)
        val manager = context.getSystemService(AlarmManager::class.java)
        manager.setAndAllowWhileIdle(
            AlarmManager.RTC_WAKEUP,
            remindAtMs,
            pendingIntent(context, scope, network, memoId),
        )
    }

    fun cancel(context: Context, scope: ReminderScopeArg, network: String, memoId: String) {
        val manager = context.getSystemService(AlarmManager::class.java)
        manager.cancel(pendingIntent(context, scope, network, memoId))
    }

    /** 端末再起動後の再登録(BOOT_COMPLETED から)。未発火分をすべて登録し直す。 */
    fun rescheduleAll(context: Context) {
        val baseDir = context.filesDir.absolutePath
        try {
            for (r in memoReminderList(baseDir)) {
                schedule(context, r.scope, r.network, r.memoId, r.remindAt.toLong())
            }
        } catch (e: MobileException) {
            Log.w("peercove", "リマインダーの再登録に失敗: ${e.message}")
        }
    }
}

/** AlarmManager の発火を受ける。inexact のため複数件がまとめて発火しうるが、
 * take_due が「発火時刻を過ぎた分すべて」を返すので自然に処理できる。 */
class ReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        Thread {
            try {
                val baseDir = context.filesDir.absolutePath
                for (reminder in memoReminderTakeDue(baseDir)) {
                    val title = resolveReminderTitle(context, baseDir, reminder) ?: continue
                    ReminderNotifier.show(context, reminder, title)
                }
            } catch (e: MobileException) {
                Log.w("peercove", "リマインダー処理に失敗: ${e.message}")
            } finally {
                pending.finish()
            }
        }.start()
    }
}

/** 端末再起動後にリマインダーを再登録する。 */
class ReminderBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        val pending = goAsync()
        Thread {
            try {
                ReminderScheduler.rescheduleAll(context)
            } finally {
                pending.finish()
            }
        }.start()
    }
}

/** 発火時のタイトル解決。削除済み・アクセス不可なら null(通知しない)。
 *
 * ADR-0055 決定 3: スケジュールの予定タイトル解決(共有スケジュールの
 * キャッシュから引く処理)は次工程(UI 実装)で行う。現段階では
 * `memo_untitled` と同じプレースホルダにフォールバックし、通知そのものは
 * 壊さない。 */
private fun resolveReminderTitle(
    context: Context,
    baseDir: String,
    reminder: MemoReminderInfo,
): String? = try {
    val title = when (reminder.scope) {
        ReminderScopeArg.SHARED -> sharedMemoGet(baseDir, reminder.network, reminder.memoId).title
        ReminderScopeArg.SCHEDULE -> context.getString(R.string.schedule_reminder_placeholder_title)
        ReminderScopeArg.PERSONAL -> memoGet(baseDir, reminder.memoId).title
    }
    title.ifEmpty { context.getString(R.string.memo_untitled) }
} catch (e: MobileException) {
    null
}

/** このメモ・予定(個人・共有・スケジュール)に設定中のリマインダー全件
 * (ADR-0055 決定 3: 1 対象に複数件になり得る)。 */
suspend fun fetchReminders(
    baseDir: String,
    scope: ReminderScopeArg,
    network: String,
    memoId: String,
): List<MemoReminderInfo> = try {
    withContext(Dispatchers.IO) { memoRemindersFor(baseDir, scope, network, memoId) }
} catch (e: MobileException) {
    emptyList()
}

/** リマインダーの設定 + AlarmManager への登録。同一 `remindAtMs` への
 * 再設定は上書き、異なる時刻は追加(1 対象につき複数件、ADR-0055 決定 3)。
 * `offsetMinutes` は「開始 n 分前」設定の表示用メタ(null = 任意日時)。 */
suspend fun applyReminder(
    context: Context,
    baseDir: String,
    scope: ReminderScopeArg,
    network: String,
    memoId: String,
    remindAtMs: Long,
    offsetMinutes: UInt? = null,
) {
    withContext(Dispatchers.IO) {
        memoReminderSet(baseDir, scope, network, memoId, remindAtMs.toULong(), offsetMinutes)
    }
    ReminderScheduler.schedule(context, scope, network, memoId, remindAtMs)
}

/** リマインダーの解除 + AlarmManager からも取り消す。`remindAtMs` を
 * 省略(null)すると、その対象の全件を削除する(ADR-0055 決定 3)。 */
suspend fun clearReminder(
    context: Context,
    baseDir: String,
    scope: ReminderScopeArg,
    network: String,
    memoId: String,
    remindAtMs: Long? = null,
) {
    withContext(Dispatchers.IO) {
        memoReminderClear(baseDir, scope, network, memoId, remindAtMs?.toULong())
    }
    ReminderScheduler.cancel(context, scope, network, memoId)
}

/** `DatePickerDialog` → `TimePickerDialog` の順で起動し、選んだ時刻(ミリ秒)を返す
 * (指示書: 日時ピッカーは DatePickerDialog + TimePickerDialog の組で可)。 */
fun pickReminderDateTime(context: Context, initialMillis: Long, onPicked: (Long) -> Unit) {
    val initial = Calendar.getInstance().apply { timeInMillis = initialMillis }
    DatePickerDialog(
        context,
        { _, year, month, day ->
            TimePickerDialog(
                context,
                { _, hour, minute ->
                    val picked = Calendar.getInstance().apply {
                        set(year, month, day, hour, minute, 0)
                        set(Calendar.MILLISECOND, 0)
                    }
                    onPicked(picked.timeInMillis)
                },
                initial.get(Calendar.HOUR_OF_DAY),
                initial.get(Calendar.MINUTE),
                true,
            ).show()
        },
        initial.get(Calendar.YEAR),
        initial.get(Calendar.MONTH),
        initial.get(Calendar.DAY_OF_MONTH),
    ).show()
}
