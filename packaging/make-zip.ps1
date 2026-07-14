# PeerCove ポータブル版(Windows ZIP)を組み立てる(M2-G7b、上級者向け配布)。
#
# 前提(先に済ませておく):
#   cargo build --release -p peercove-cli
#   cd apps\peercove-ui; npm install; npm run tauri build   (UI 実行ファイルを作る)
#   apps\peercove-ui\src-tauri\windows\wintun.dll を配置(wintun.net の署名済み)
#   packaging\licenses\wintun-LICENSE.txt を配置(同 zip の LICENSE.txt)
#
# 使い方(リポジトリのどこからでも):
#   powershell -ExecutionPolicy Bypass -File packaging\make-zip.ps1
#
# 出力: packaging\dist\PeerCove-portable-windows-x64.zip

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot   # packaging の 1 つ上 = リポジトリルート

function Require-File([string]$path, [string]$hint) {
    if (-not (Test-Path $path)) {
        throw "見つかりません: $path`n  → $hint"
    }
    return $path
}

$daemon = Require-File "$root\target\release\peercove.exe" `
    "先に `cargo build --release -p peercove-cli` を実行してください"
$wintun = Require-File "$root\apps\peercove-ui\src-tauri\windows\wintun.dll" `
    "wintun.net の zip の bin\amd64\wintun.dll をここへ配置してください(packaging\licenses\README.md)"
$license = Require-File "$root\packaging\licenses\wintun-LICENSE.txt" `
    "wintun の zip の LICENSE.txt を packaging\licenses\wintun-LICENSE.txt へ配置してください"
$portableReadme = Require-File "$root\packaging\README-portable.md" "リポジトリの packaging にあるはず"

# UI 実行ファイル名は cargo/tauri で異なりうるので候補を順に探す
$uiCandidates = @(
    "$root\apps\peercove-ui\src-tauri\target\release\PeerCove.exe",
    "$root\apps\peercove-ui\src-tauri\target\release\peercove-ui.exe"
)
$ui = $uiCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $ui) {
    throw "UI 実行ファイルが見つかりません(探した場所: $($uiCandidates -join ', '))`n" +
          "  → apps\peercove-ui で `npm run tauri build` を実行してください"
}

# 組み立て
$stage = "$root\packaging\dist\stage-win"
$out = "$root\packaging\dist\PeerCove-portable-windows-x64.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item $daemon "$stage\peercove.exe"
Copy-Item $ui "$stage\PeerCove.exe"
Copy-Item $wintun "$stage\wintun.dll"
Copy-Item $license "$stage\wintun-LICENSE.txt"
Copy-Item $portableReadme "$stage\README.md"

if (Test-Path $out) { Remove-Item -Force $out }
Compress-Archive -Path "$stage\*" -DestinationPath $out
Remove-Item -Recurse -Force $stage

Write-Host "作成しました: $out"
Write-Host "内容: peercove.exe / PeerCove.exe / wintun.dll / wintun-LICENSE.txt / README.md"
