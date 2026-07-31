# SPDX-License-Identifier: GPL-2.0-or-later
#
# Builds the three vgmtools optimisers (vgm_cmp, vgm_sro, optdac) as
# wasm32-wasip1 command modules into target/wasi-tools/, for the web pack
# export and the byte-parity test.
#
# These are unmodified POSIX C programs, so they compile against the real
# wasi-libc rather than a hand-written shim -- the standard route for existing
# C CLI tools (see OPTIMIZER-WASM-PLAN.md, "the wasip1 pivot"). The host runs
# them like processes: argv in, files through a preopened directory, an exit
# code out. Only `shim/zlib.h`/`zshim.c` ride along, serving the tools' gz*
# calls from plain FILE* exactly as the native build does.
#
# Toolchain: the Scoop/stock clang already in PATH plus two artifacts from the
# wasi-sdk release -- the sysroot (headers + wasi-libc) and the compiler-rt
# builtins clang does not ship for wasm32. Both are downloaded once and cached
# under target/wasi (the CJK-font pattern in build-web.ps1): clang is driven
# with `-nodefaultlibs` and explicit paths, so nothing is installed into the
# clang directory itself.
#
# Works in Windows PowerShell 5.1 and pwsh (CI).

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$cache = Join-Path $root "target\wasi"
$out = Join-Path $root "target\wasi-tools"
$upstream = Join-Path $root "vendor\upstream\vgmtools"
$shim = Join-Path $root "crates\vgms-vgmtools\shim"

# The pinned wasi-sdk release these artifacts come from. Bumping it is: change
# the version, delete target/wasi, rebuild, and re-run the parity test.
$wasiVersion = "33.0+m"
$wasiTag = "wasi-sdk-33"
$sysroot = Join-Path $cache "wasi-sysroot-$wasiVersion"
$builtins = Join-Path $cache "libclang_rt-$wasiVersion\wasm32-unknown-wasip1\libclang_rt.builtins.a"

if (-not (Get-Command clang -ErrorAction SilentlyContinue)) {
    throw "clang is not on PATH (any clang with the wasm32 backend will do)"
}

# --- fetch + cache the sysroot and builtins (once) -------------------------
$base = "https://github.com/WebAssembly/wasi-sdk/releases/download/$wasiTag"
$escaped = $wasiVersion.Replace("+", "%2B")

if (-not (Test-Path $sysroot)) {
    New-Item -ItemType Directory -Force $cache | Out-Null
    $tar = Join-Path $cache "wasi-sysroot.tar.gz"
    Write-Host "Downloading the wasi sysroot ($wasiTag, ~120 MB, once)..."
    Invoke-WebRequest -Uri "$base/wasi-sysroot-$escaped.tar.gz" -OutFile $tar -UseBasicParsing -TimeoutSec 600
    tar -xzf $tar -C $cache
    if ($LASTEXITCODE -ne 0) { throw "extracting the wasi sysroot failed" }
    Remove-Item $tar
}
if (-not (Test-Path $builtins)) {
    New-Item -ItemType Directory -Force $cache | Out-Null
    $tar = Join-Path $cache "libclang_rt.tar.gz"
    Write-Host "Downloading the wasm32 compiler-rt builtins ($wasiTag, ~640 KB, once)..."
    Invoke-WebRequest -Uri "$base/libclang_rt-$escaped.tar.gz" -OutFile $tar -UseBasicParsing -TimeoutSec 600
    tar -xzf $tar -C $cache
    if ($LASTEXITCODE -ne 0) { throw "extracting the builtins failed" }
    Remove-Item $tar
}

# --- compile the three tools ----------------------------------------------
New-Item -ItemType Directory -Force $out | Out-Null

# (module name, sources) -- the module names are what pack_worker.js fetches.
$tools = @(
    @{ Name = "tool_vgm_cmp"; Sources = @("vgm_cmp.c", "chip_cmp.c") },
    @{ Name = "tool_vgm_sro"; Sources = @("vgm_sro.c", "chip_srom.c") },
    @{ Name = "tool_optdac";  Sources = @("optdac.c") }
)

foreach ($tool in $tools) {
    # @(...) so a single-source tool still splats as an array, not char-by-char.
    $sources = @($tool.Sources | ForEach-Object { Join-Path $upstream $_ })
    $wasm = Join-Path $out "$($tool.Name).wasm"
    Write-Host "  $($tool.Name).wasm"
    # -nodefaultlibs + explicit -lc/builtins: clang's driver would otherwise
    # look for the builtins inside its own installation, which stock clang
    # does not ship for wasm32. --stack-first makes a stack overflow trap
    # instead of silently corrupting globals; the 256 MiB --max-memory turns a
    # runaway allocation into a trap instead of a dead tab (the ceiling the
    # plan promised, which no native child process has).
    & clang --target=wasm32-wasip1 --sysroot="$sysroot" -O2 -w `
        -I $shim -I $upstream `
        @sources (Join-Path $shim "zshim.c") `
        -nodefaultlibs -lc "$builtins" `
        "-Wl,--stack-first" "-Wl,-z,stack-size=1048576" "-Wl,--max-memory=268435456" `
        -o $wasm
    if ($LASTEXITCODE -ne 0) { throw "building $($tool.Name) failed" }
}

Write-Host "wasi tool modules ready in $out"
