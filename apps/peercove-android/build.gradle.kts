// ルートビルド: プラグインのバージョンをここで一元管理する
plugins {
    id("com.android.application") version "8.11.1" apply false
    // 2.2 系: Markdown レンダラ(Kotlin 2.3 でビルド)のメタデータを読むため
    // (2.1 コンパイラは metadata 2.2 までしか読めない)
    id("org.jetbrains.kotlin.android") version "2.2.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.2.21" apply false
}
