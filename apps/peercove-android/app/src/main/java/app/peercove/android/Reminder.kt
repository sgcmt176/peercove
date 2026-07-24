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
// あえて残してある。スケジュールの予定リマインダー(実装順 H-3/H-6)で流用する
// ため。既に設定済みのメモリマインダーは引き続き発火する(害はない)。
//
// M6 H-6: PendingIntent の requestCode(= 通知 ID)は remindAt も含めた
// ハッシュにしてある(H-3a 申し送りの既知の穴の解消)。同一予定(memoId)に
// 複数のリマインダーを設定しても、時刻ごとに別の AlarmManager 登録・別の
// 通知として扱われる。Boot 再登録(rescheduleAll)は memoReminderList の
// 各行をそのまま schedule() し直すだけで、この変更に自然に追随する。

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
import uniffi.peercove_mobile.scheduleList
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

    /** M6 H-6: remindAt も含める(同一対象の複数リマインダーを区別するため。
     * H-3a 申し送りの既知の穴の解消)。 */
    fun notificationId(scope: String, network: String, memoId: String, remindAt: Long): Int =
        "$scope:$network:$memoId:$remindAt".hashCode()

    /** タイトルはローカル通知への表示であり、ログではない(ADR-0049)。
     * scope SCHEDULE だけ表示文言が異なる(「⏰ 予定: <タイトル>」、M6 H-6)。 */
    fun show(context: Context, reminder: MemoReminderInfo, title: String) {
        ensureChannel(context)
        val scopeStr = reminder.scope.wire()
        val id = notificationId(scopeStr, reminder.network, reminder.memoId, reminder.remindAt.toLong())
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
        val contentTitle = if (reminder.scope == ReminderScopeArg.SCHEDULE) {
            context.getString(R.string.notif_schedule_reminder_title, title)
        } else {
            context.getString(R.string.notif_reminder_title, title)
        }
        val notification = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(contentTitle)
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
        remindAtMs: Long,
    ): PendingIntent {
        val scopeStr = scope.wire()
        val requestCode = ReminderNotifier.notificationId(scopeStr, network, memoId, remindAtMs)
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

    /** 登録(inexact)。過去時刻の拒否はストア側(memoReminderSet)が担う。
     * requestCode に remindAtMs を含めるため、同一対象の複数リマインダーは
     * 互いに上書きしない(M6 H-6)。 */
    fun schedule(context: Context, scope: ReminderScopeArg, network: String, memoId: String, remindAtMs: Long) {
        ReminderNotifier.ensureChannel(context)
        val manager = context.getSystemService(AlarmManager::class.java)
        manager.setAndAllowWhileIdle(
            AlarmManager.RTC_WAKEUP,
            remindAtMs,
            pendingIntent(context, scope, network, memoId, remindAtMs),
        )
    }

    /** `remindAtMs` を指定した 1 件だけを取り消す(同一対象の他のリマインダーは
     * 残る、M6 H-6)。 */
    fun cancel(context: Context, scope: ReminderScopeArg, network: String, memoId: String, remindAtMs: Long) {
        val manager = context.getSystemService(AlarmManager::class.java)
        manager.cancel(pendingIntent(context, scope, network, memoId, remindAtMs))
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
 * M6 H-6: スケジュールの予定タイトルは `scheduleList`(キャッシュ、ホスト
 * 未接続でも読める)から id 一致で引く。見つからなければ(削除済みなど)
 * 通知そのものを出さない(タイトルが分からないまま「⏰ 予定: (予定)」を
 * 出すより、解決できないときは黙って通知しない = 個人メモ・共有メモと同じ
 * 方針に揃える)。 */
private fun resolveReminderTitle(
    context: Context,
    baseDir: String,
    reminder: MemoReminderInfo,
): String? = try {
    val title = when (reminder.scope) {
        ReminderScopeArg.SHARED -> sharedMemoGet(baseDir, reminder.network, reminder.memoId).title
        ReminderScopeArg.SCHEDULE -> {
            val event = scheduleList(baseDir, reminder.network).events
                .firstOrNull { it.id == reminder.memoId } ?: return null
            event.title
        }
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
 * 省略(null)すると、その対象の全件を削除する(ADR-0055 決定 3)。
 * M6 H-6: AlarmManager 側は remindAt ごとに別登録なので、全件削除のときは
 * 消す前に一覧を読んでおき、1 件ずつ取り消す。 */
suspend fun clearReminder(
    context: Context,
    baseDir: String,
    scope: ReminderScopeArg,
    network: String,
    memoId: String,
    remindAtMs: Long? = null,
) {
    val toCancel = if (remindAtMs != null) {
        listOf(remindAtMs)
    } else {
        withContext(Dispatchers.IO) { memoRemindersFor(baseDir, scope, network, memoId) }
            .map { it.remindAt.toLong() }
    }
    withContext(Dispatchers.IO) {
        memoReminderClear(baseDir, scope, network, memoId, remindAtMs?.toULong())
    }
    for (at in toCancel) {
        ReminderScheduler.cancel(context, scope, network, memoId, at)
    }
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
