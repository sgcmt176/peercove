// PeerCove Android(M4、ADR-0039)。メンバー専用アプリ。
// リポジトリ内の Rust コア(crates/peercove-mobile)を UniFFI 経由で使う。
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "peercove-android"
include(":app")
