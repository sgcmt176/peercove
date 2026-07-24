package app.peercove.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent

/**
 * 共有メモのコメント・メンションの OS 通知(M6 H-2、ADR-0055 決定 1)。
 * F-5 で見送っていた Android 側のコメント通知をここで解消する。
 *
 * 検出は [PeercoveVpnService.startCommentWatcher](デスクトップの notify.ts と
 * 同じ、comment_count の差分ポーリング)。ここは ChatNotifier / ReminderNotifier
 * と同じ流儀の単純な通知(タップでアプリを開くだけ。メモへの直接遷移は
 * 持たない — チャットお知らせ行の `@memo:id` カードから開ける)。
 */
object MemoCommentNotifier {
    const val CHANNEL_ID = "peercove_memo_comment"

    fun ensureChannel(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(
            CHANNEL_ID,
            context.getString(R.string.notif_comment_channel),
            NotificationManager.IMPORTANCE_HIGH,
        )
        manager.createNotificationChannel(channel)
    }

    private fun notificationId(slug: String, memoId: String, commentId: String): Int =
        "$slug:$memoId:$commentId".hashCode()

    /** タイトル・本文はローカル通知への表示でありログではない(ADR-0049)。 */
    fun show(
        context: Context,
        slug: String,
        memoId: String,
        commentId: String,
        title: String,
        body: String,
    ) {
        ensureChannel(context)
        val id = notificationId(slug, memoId, commentId)
        val open = PendingIntent.getActivity(
            context,
            id,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(ChatNotifier.EXTRA_SLUG, slug),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(id, notification)
    }
}
