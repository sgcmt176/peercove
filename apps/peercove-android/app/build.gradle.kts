import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// ---- Rust 連携 -------------------------------------------------------------
// ビルドの流れ: cargo ndk(.so を jniLibs へ)→ uniffi-bindgen(Kotlin 生成)
// → 通常の Android ビルド。どちらの成果物も生成物なのでコミットしない。

// apps/peercove-android/app → リポジトリルート
val repoRoot: File = rootDir.parentFile.parentFile

// cargo は PATH に無い環境があるので ~/.cargo/bin を直接探す
fun cargoExe(): String {
    val home = System.getProperty("user.home")
    val candidates = listOf(File(home, ".cargo/bin/cargo.exe"), File(home, ".cargo/bin/cargo"))
    return candidates.firstOrNull { it.exists() }?.absolutePath ?: "cargo"
}

// cargo-ndk が NDK を見つけられるよう、local.properties の sdk.dir を環境変数へ流す
fun sdkDirFromLocalProperties(): String? {
    val f = File(rootDir, "local.properties")
    if (!f.exists()) return null
    val p = Properties()
    f.inputStream().use { p.load(it) }
    return p.getProperty("sdk.dir")
}

// ---- リリース署名 ----------------------------------------------------------
// リリース APK は自己生成の鍵で署名して sideload 配布する(Play ストアは未対応)。
// 鍵本体とパスワードはリポジトリに入れないので、apps/peercove-android/
// keystore.properties(gitignore 済み)から読む。ファイルが無ければ署名設定を
// 付けない = 未署名 APK。debug ビルドと CI は従来どおり通る。
// 作り方は docs/development.md「リリース APK の署名」。
//
// **署名鍵を変えると既存インストールを上書き更新できない**(Android が拒否する。
// 利用者はアンインストールが必要 = 設定・チャット・メモが消える)。一度配布した
// 鍵とパスワードは必ず保管し続けること。
fun releaseSigningProps(): Properties? {
    val f = File(rootDir, "keystore.properties")
    if (!f.exists()) return null
    val p = Properties()
    f.inputStream().use { p.load(it) }
    return p
}

// storeFile は絶対パス(リポジトリ外に置く運用を推奨)と、rootDir 相対の両方を許す
fun resolveKeystoreFile(path: String): File {
    val f = File(path)
    return if (f.isAbsolute) f else File(rootDir, path)
}

val jniLibsDir: File = file("src/main/jniLibs")
val uniffiOutDir: Provider<Directory> = layout.buildDirectory.dir("generated/uniffi")

val cargoNdkBuild = tasks.register<Exec>("cargoNdkBuild") {
    group = "rust"
    description = "crates/peercove-mobile を Android(arm64)向けにビルドして jniLibs へ配置"
    workingDir = repoRoot
    sdkDirFromLocalProperties()?.let { environment("ANDROID_HOME", it) }
    commandLine(
        cargoExe(), "ndk", "-t", "arm64-v8a",
        "-o", jniLibsDir.absolutePath,
        "build", "-p", "peercove-mobile", "--release",
    )
}

val generateUniffiBindings = tasks.register<Exec>("generateUniffiBindings") {
    group = "rust"
    description = "コンパイル済み .so から Kotlin バインディングを生成(uniffi ライブラリモード)"
    dependsOn(cargoNdkBuild)
    workingDir = repoRoot
    commandLine(
        cargoExe(), "run", "-p", "peercove-mobile", "--bin", "uniffi-bindgen", "--",
        "generate",
        "--library", File(jniLibsDir, "arm64-v8a/libpeercove_mobile.so").absolutePath,
        "--language", "kotlin",
        "--out-dir", uniffiOutDir.get().asFile.absolutePath,
    )
}

tasks.named("preBuild") { dependsOn(generateUniffiBindings) }

// ---- Android ---------------------------------------------------------------

android {
    namespace = "app.peercove.android"
    compileSdk = 36
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "app.peercove.android"
        minSdk = 26
        targetSdk = 36
        versionCode = 18
        versionName = "0.1.2"
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    val signingProps = releaseSigningProps()

    signingConfigs {
        if (signingProps != null) {
            create("release") {
                val storePath = requireNotNull(signingProps.getProperty("storeFile")) {
                    "keystore.properties に storeFile がありません"
                }
                val store = resolveKeystoreFile(storePath)
                require(store.exists()) { "キーストアが見つかりません: ${store.absolutePath}" }
                storeFile = store
                storePassword = signingProps.getProperty("storePassword")
                keyAlias = signingProps.getProperty("keyAlias")
                keyPassword = signingProps.getProperty("keyPassword")
                // v3 を有効にしておくと、将来どうしても鍵を替えたくなったときに
                // 「鍵のローテーション」で上書き更新できる余地が残る(Android 9+)。
                // minSdk 26 なので v1(JAR 署名)は不要。
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // keystore.properties が無い環境では未署名のまま(端末には入らない)
            signingProps?.let { signingConfig = signingConfigs.getByName("release") }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
    }

    sourceSets["main"].kotlin.srcDir(uniffiOutDir)
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.05.00")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")

    // UniFFI 生成コードの実行時依存(JNA は Android では aar 版を使う)
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")

    // 招待 QR コードの読み取り(M4 E-B)
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
}
