package app.peercove.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Person
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import uniffi.peercove_mobile.ChatMessage
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.chatGroups
import uniffi.peercove_mobile.sendChatMessage

/**
 * チャットの新着通知(E-E 2、VPN 接続中のみ)。
 * 会話ごとに 1 つの通知(MessagingStyle)を出し、通知から直接返信・既読にできる。
 * サーバーレス構成のため、VPN サービスが動いていない間の通知は仕様として無い。
 */
object ChatNotifier {
    const val CHANNEL_ID = "peercove_chat"
    const val ACTION_REPLY = "app.peercove.android.action.CHAT_REPLY"
    const val ACTION_MARK_READ = "app.peercove.android.action.CHAT_MARK_READ"
    const val EXTRA_SLUG = "slug"
    const val EXTRA_CONV = "conv"
    const val EXTRA_LATEST_SEQ = "latest_seq"
    const val KEY_REPLY_TEXT = "reply_text"

    fun ensureChannel(context: Context) {
        val channel = NotificationChannel(
            CHANNEL_ID,
            context.getString(R.string.notif_chat_channel),
            NotificationManager.IMPORTANCE_DEFAULT,
        )
        context.getSystemService(NotificationManager::class.java)
            .createNotificationChannel(channel)
    }

    fun notificationId(convId: String): Int = convId.hashCode()

    /** 会話 ID(NetworkScreen の storageId と同じ形式)。 */
    fun convIdOf(message: ChatMessage): String = when (message.scope) {
        "direct" -> "direct/${message.fromIp}"
        "group" -> "group/${message.groupId}"
        else -> "network"
    }

    /** 会話の表示名(グループは GroupStore から引く)。 */
    private fun titleOf(context: Context, slug: String, convId: String, last: ChatMessage): String =
        when {
            convId == "network" -> context.getString(R.string.talk_all)
            convId.startsWith("group/") -> {
                val id = convId.removePrefix("group/")
                chatGroups(slug).firstOrNull { it.id == id }?.name
                    ?: context.getString(R.string.talk_all)
            }
            else -> last.fromName
        }

    private fun bodyOf(context: Context, message: ChatMessage): String =
        message.fileName?.let { context.getString(R.string.chat_file_prefix, it) } ?: message.text

    /** 会話 1 つぶんの通知を出す(未読メッセージの末尾 6 件)。 */
    fun show(context: Context, slug: String, convId: String, unread: List<ChatMessage>) {
        val last = unread.last()
        val latestSeq = last.seq.toLong()
        val title = titleOf(context, slug, convId, last)

        val style = Notification.MessagingStyle(
            Person.Builder().setName(context.getString(R.string.badge_self)).build(),
        )
        style.conversationTitle = title
        for (message in unread.takeLast(6)) {
            style.addMessage(
                bodyOf(context, message),
                message.sentAt.toLong(),
                Person.Builder().setName(message.fromName).build(),
            )
        }

        fun actionIntent(action: String): PendingIntent = PendingIntent.getBroadcast(
            context,
            notificationId(convId),
            Intent(context, ChatActionReceiver::class.java)
                .setAction(action)
                .putExtra(EXTRA_SLUG, slug)
                .putExtra(EXTRA_CONV, convId)
                .putExtra(EXTRA_LATEST_SEQ, latestSeq),
            // RemoteInput を運ぶため MUTABLE(返信)。既読側も同じでよい
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
        )

        val replyAction = Notification.Action.Builder(
            null,
            context.getString(R.string.notif_reply),
            actionIntent(ACTION_REPLY),
        )
            .addRemoteInput(
                RemoteInput.Builder(KEY_REPLY_TEXT)
                    .setLabel(context.getString(R.string.chat_placeholder))
                    .build(),
            )
            .build()
        val markReadAction = Notification.Action.Builder(
            null,
            context.getString(R.string.notif_mark_read),
            actionIntent(ACTION_MARK_READ),
        ).build()

        val open = PendingIntent.getActivity(
            context,
            notificationId(convId),
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_SLUG, slug)
                .putExtra(EXTRA_CONV, convId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val notification = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setStyle(style)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setOnlyAlertOnce(false)
            .addAction(replyAction)
            .addAction(markReadAction)
            .build()
        context.getSystemService(NotificationManager::class.java)
            .notify(notificationId(convId), notification)
    }

    fun cancel(context: Context, convId: String) {
        context.getSystemService(NotificationManager::class.java)
            .cancel(notificationId(convId))
    }
}

/** 通知アクション(返信・既読)の受け口。 */
class ChatActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val slug = intent.getStringExtra(ChatNotifier.EXTRA_SLUG) ?: return
        val convId = intent.getStringExtra(ChatNotifier.EXTRA_CONV) ?: return
        val latestSeq = intent.getLongExtra(ChatNotifier.EXTRA_LATEST_SEQ, 0L)
        when (intent.action) {
            ChatNotifier.ACTION_MARK_READ -> {
                Prefs.setReadSeq(context, slug, convId, latestSeq)
                ChatNotifier.cancel(context, convId)
            }
            ChatNotifier.ACTION_REPLY -> {
                val text = RemoteInput.getResultsFromIntent(intent)
                    ?.getCharSequence(ChatNotifier.KEY_REPLY_TEXT)?.toString()?.trim()
                if (text.isNullOrEmpty()) return
                val pending = goAsync()
                Thread {
                    try {
                        when {
                            convId == "network" ->
                                sendChatMessage(slug, "network", null, null, text)
                            convId.startsWith("group/") -> sendChatMessage(
                                slug, "group", null, convId.removePrefix("group/"), text,
                            )
                            else -> sendChatMessage(
                                slug, "direct", convId.removePrefix("direct/"), null, text,
                            )
                        }
                        Prefs.setReadSeq(context, slug, convId, latestSeq)
                        ChatNotifier.cancel(context, convId)
                    } catch (e: MobileException) {
                        Log.w("peercove", "通知からの返信に失敗: ${e.message}")
                    } finally {
                        pending.finish()
                    }
                }.start()
            }
        }
    }
}
