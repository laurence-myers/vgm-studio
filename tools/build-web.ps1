# SPDX-License-Identifier: GPL-2.0-or-later
#
# Builds the web target into target/web-dist: a servable directory with the app
# module (wasm-bindgen glue + wasm), the AudioWorklet module, the Worker and
# processor scripts, the page, and the licence notices.
#
# Prerequisites: the wasm32-unknown-unknown target (rustup adds it from
# rust-toolchain.toml) and `wasm-bindgen` on PATH at the version Cargo.lock pins
# (0.2.126) -- `cargo install wasm-bindgen-cli --version 0.2.126 --locked`.
#
# No new build system: this is the whole pipeline, said out loud.
#
# -E2e builds the app module with the `e2e` feature, so the page installs the
# `window.__vgms_e2e` action/state hook the Playwright suite drives (wt-6). A
# normal (release) build never passes it, so the hook never ships.

param(
    [switch]$E2e
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root "target\wasm32-unknown-unknown\release"
$dist = Join-Path $root "target\web-dist"

$appFeatures = @()
if ($E2e) {
    Write-Host "e2e build: enabling the vgms-web `e2e` feature (window.__vgms_e2e)."
    $appFeatures = @("--features", "e2e")
}

Write-Host "Building the AudioWorklet module (vgms-synth-worklet)..."
cargo build -p vgms-synth-worklet --lib --target wasm32-unknown-unknown --release
if ($LASTEXITCODE -ne 0) { throw "worklet build failed" }

Write-Host "Building the app module (vgms-web)..."
cargo build -p vgms-web --lib --target wasm32-unknown-unknown --release @appFeatures
if ($LASTEXITCODE -ne 0) { throw "app module build failed" }

Write-Host "Building the vgmtools optimiser modules (vgm_cmp/vgm_sro/optdac)..."
# wasm32-wasip1 command modules -- unmodified C over the real wasi-libc, built
# outside cargo; the script fetches and caches its own sysroot on first run.
& (Join-Path $PSScriptRoot "build-wasi-tools.ps1")

Write-Host "Running wasm-bindgen over the app module..."
New-Item -ItemType Directory -Force $dist | Out-Null
wasm-bindgen --target web --no-typescript --out-dir $dist --out-name vgms_web `
    (Join-Path $release "vgms_web.wasm")
if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed (is it installed at 0.2.126?)" }

Write-Host "Assembling target/web-dist..."
# The AudioWorklet module the processor instantiates from bytes.
Copy-Item (Join-Path $release "vgms_synth_worklet.wasm") (Join-Path $dist "vgms_synth_worklet.wasm") -Force
# The three optimiser modules the pack worker fetches and instantiates per song.
$wasiTools = Join-Path $root "target\wasi-tools"
foreach ($tool in @("tool_vgm_cmp", "tool_vgm_sro", "tool_optdac")) {
    Copy-Item (Join-Path $wasiTools "$tool.wasm") (Join-Path $dist "$tool.wasm") -Force
}
# The page, the Worker bootstrap, and the AudioWorklet processor.
Copy-Item (Join-Path $root "web\*") $dist -Recurse -Force
# The licences: the distributed bundle is GPL-2.0-or-later, same as the exe.
Copy-Item (Join-Path $root "licenses") (Join-Path $dist "licenses") -Recurse -Force

# The CJK fallback font (for Japanese GD3 tags). The web build has no system
# fonts, so it fetches this at runtime; here it is downloaded once into a cache
# and copied beside the module. Best-effort: a failed download just means CJK
# text renders as boxes, exactly as before. Noto Sans JP (SIL OFL), ~4.5 MB.
$cjkCache = Join-Path $root "target\cjk-font.otf"
$cjkUrl = "https://cdn.jsdelivr.net/npm/@expo-google-fonts/noto-sans-jp@0.2.3/NotoSansJP_400Regular.ttf"
if (-not (Test-Path $cjkCache)) {
    Write-Host "Downloading the CJK fallback font (once, ~4.5 MB)..."
    try {
        Invoke-WebRequest -Uri $cjkUrl -OutFile $cjkCache -UseBasicParsing -TimeoutSec 180
    } catch {
        Write-Warning "CJK font download failed; Japanese text will show as boxes on the web. ($_)"
    }
}
if (Test-Path $cjkCache) {
    Copy-Item $cjkCache (Join-Path $dist "cjk-font.otf") -Force
}

Write-Host "`nweb-dist contents:"
Get-ChildItem $dist -File |
    Where-Object { $_.Extension -in ".wasm", ".js", ".html" } |
    Sort-Object Length -Descending |
    ForEach-Object { "{0,12:N0}  {1}" -f $_.Length, $_.Name }

Write-Host "`nServable web target ready at $dist"
