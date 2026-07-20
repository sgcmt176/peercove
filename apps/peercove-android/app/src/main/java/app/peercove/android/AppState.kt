package app.peercove.android

/**
 * アプリ(Activity)の表示状態。VPN サービスのチャット通知が
 * 「アプリを見ている間は通知を出さない」ために参照する。
 */
object AppState {
    @Volatile
    var visible: Boolean = false
}
