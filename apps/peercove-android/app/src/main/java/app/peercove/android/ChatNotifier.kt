package app.peercove.android

import android.app.KeyguardManager
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
    // v1 は IMPORTANCE_DEFAULT で作ってしまいバナー(ヘッズアップ)が出ない。
    // チャンネル設定は端末に固定されるため ID を切り替えて作り直す
    const val CHANNEL_ID = "peercove_chat_v2"
    private const val OLD_CHANNEL_ID = "peercove_chat"
    const val ACTION_REPLY = "app.peercove.android.action.CHAT_REPLY"
    const val ACTION_MARK_READ = "app.peercove.android.action.CHAT_MARK_READ"
    const val EXTRA_SLUG = "slug"
    const val EXTRA_CONV = "conv"
    const val EXTRA_LATEST_SEQ = "latest_seq"
    const val KEY_REPLY_TEXT = "reply_text"

    fun ensureChannel(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        // HIGH = バナー(ヘッズアップ)表示 + 音。アプリを閉じていても気付ける
        val channel = NotificationChannel(
            CHANNEL_ID,
            context.getString(R.string.notif_chat_channel),
            NotificationManager.IMPORTANCE_HIGH,
        )
        manager.createNotificationChannel(channel)
        manager.deleteNotificationChannel(OLD_CHANNEL_ID)
    }

    fun notificationId(convId: String): Int = convId.hashCode()

    /** 直近に通知した (最新 seq, ロックで隠したか)。同じ内容の再表示で
     *  音を鳴らし直さない(チャット監視は 3 秒ごとに show を呼ぶため)。
     *  ロック状態が変わったら(解錠したら)本文入りへ差し替えて再表示する。
     *  監視スレッドと通知アクション(別スレッド)から触るので並行対応。 */
    private val lastShown = java.util.concurrent.ConcurrentHashMap<String, Pair<Long, Boolean>>()

    private fun deviceLocked(context: Context): Boolean =
        context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked == true

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
        if (Prefs.isMuted(context, slug, convId)) return
        val last = unread.last()
        val latestSeq = last.seq.toLong()

        // 「ロック画面で本文を隠す」= 端末がロック中は本文を出さない。
        // VISIBILITY_PRIVATE はシステムの「ロック画面: すべて表示」設定に
        // 負けて隠れないため、KeyguardManager で自前に判定して差し替える。
        val hidden = Prefs.hideNotifContent(context) && deviceLocked(context)

        // 同じ (最新 seq, 隠す/隠さない) の再表示なら鳴らし直さない
        // (チャット監視は 3 秒ごとに show を呼ぶ)。解錠すれば hidden が
        // 変わるので、そのタイミングで本文入りへ差し替わる。
        val key = latestSeq to hidden
        if (lastShown[convId] == key) return
        lastShown[convId] = key

        val open = PendingIntent.getActivity(
            context,
            notificationId(convId),
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_SLUG, slug)
                .putExtra(EXTRA_CONV, convId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val notification = if (hidden) {
            // ロック中: 件名も本文も出さず「新着があること」だけ知らせる
            Notification.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_tile)
                .setContentTitle(context.getString(R.string.app_name))
                .setContentText(context.getString(R.string.notif_chat_hidden))
                .setContentIntent(open)
                .setAutoCancel(true)
                .setVisibility(Notification.VISIBILITY_PUBLIC) // これ自体は無害
                .build()
        } else {
            buildFull(context, slug, convId, unread, latestSeq, open)
        }
        context.getSystemService(NotificationManager::class.java)
            .notify(notificationId(convId), notification)
    }

    /** 本文入り(MessagingStyle + 返信・既読アクション)の通知。 */
    private fun buildFull(
        context: Context,
        slug: String,
        convId: String,
        unread: List<ChatMessage>,
        latestSeq: Long,
        open: PendingIntent,
    ): Notification {
        val last = unread.last()
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

        return Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setStyle(style)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setOnlyAlertOnce(false)
            .addAction(replyAction)
            .addAction(markReadAction)
            .build()
    }

    fun cancel(context: Context, convId: String) {
        lastShown.remove(convId)
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
