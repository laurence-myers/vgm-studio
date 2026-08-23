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
# Toolchain: the full wasi-sdk release bundle for the host platform -- its own
# clang, wasm-ld, sysroot (headers + wasi-libc) and compiler-rt builtins, all
# from one release so the linker and the sysroot can never disagree. (Linking
# the stock apt/Scoop clang+lld against a newer wasi-sdk sysroot fails with
# `undefined symbol: __wasm_first_page_end`, a symbol only wasm-ld from the
# matching LLVM synthesises.) The bundle is downloaded once and cached under
# target/wasi (the CJK-font pattern in build-web.ps1); nothing is installed
# outside that directory.
#
# Works in Windows PowerShell 5.1 and pwsh, on Windows and on Linux (CI).
# Child paths use forward slashes: tar and clang receive these strings
# verbatim, and on Linux a backslash is a filename character, not a separator.

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$cache = Join-Path $root "target/wasi"
$out = Join-Path $root "target/wasi-tools"
$upstream = Join-Path $root "vendor/upstream/vgmtools"
$shim = Join-Path $root "crates/vgms-vgmtools/shim"

# The pinned wasi-sdk release this bundle comes from. Bumping it is: change the
# version, delete target/wasi, rebuild, and re-run the parity test. The bundle
# assets are versioned "33.0" (the standalone sysroot carries a "+m" suffix the
# full bundle does not).
$wasiVersion = "33.0"
$wasiTag = "wasi-sdk-33"

# Host platform -> the bundle asset suffix, e.g. "x86_64-linux". Works in both
# Windows PowerShell 5.1 (.NET Framework) and pwsh (.NET Core) via the runtime
# introspection API, which exists on both.
$rt = [System.Runtime.InteropServices.RuntimeInformation]
$plat = [System.Runtime.InteropServices.OSPlatform]
if ($rt::IsOSPlatform($plat::Windows)) { $os = "windows"; $clangExe = "clang.exe" }
elseif ($rt::IsOSPlatform($plat::OSX)) { $os = "macos"; $clangExe = "clang" }
else { $os = "linux"; $clangExe = "clang" }
$archName = $rt::OSArchitecture.ToString()
$arch = switch ($archName) {
    "X64"   { "x86_64" }
    "Arm64" { "arm64" }
    default { throw "no wasi-sdk bundle for CPU architecture '$archName'" }
}
$platform = "$arch-$os"

$sdk = Join-Path $cache "wasi-sdk-$wasiVersion-$platform"
$clang = Join-Path $sdk "bin/$clangExe"
$sysroot = Join-Path $sdk "share/wasi-sysroot"

# --- fetch + cache the toolchain bundle (once) -----------------------------
$base = "https://github.com/WebAssembly/wasi-sdk/releases/download/$wasiTag"

if (-not (Test-Path $clang)) {
    New-Item -ItemType Directory -Force $cache | Out-Null
    $tar = Join-Path $cache "wasi-sdk.tar.gz"
    Write-Host "Downloading the wasi-sdk toolchain ($wasiTag, $platform, ~180 MB, once)..."
    Invoke-WebRequest -Uri "$base/wasi-sdk-$wasiVersion-$platform.tar.gz" -OutFile $tar -UseBasicParsing -TimeoutSec 600
    tar -xzf $tar -C $cache
    if ($LASTEXITCODE -ne 0) { throw "extracting the wasi-sdk toolchain failed" }
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
    # The wasi-sdk clang ships wasm32 libc and compiler-rt and finds its own
    # sysroot, so no -nodefaultlibs/explicit-builtins dance is needed (--sysroot
    # is passed only to pin it to the cached bundle). --stack-first makes a stack
    # overflow trap instead of silently corrupting globals; the 256 MiB
    # --max-memory turns a runaway allocation into a trap instead of a dead tab
    # (the ceiling the plan promised, which no native child process has).
    & $clang --target=wasm32-wasip1 --sysroot="$sysroot" -O2 -w `
        -I $shim -I $upstream `
        @sources (Join-Path $shim "zshim.c") `
        "-Wl,--stack-first" "-Wl,-z,stack-size=1048576" "-Wl,--max-memory=268435456" `
        -o $wasm
    if ($LASTEXITCODE -ne 0) { throw "building $($tool.Name) failed" }
}

Write-Host "wasi tool modules ready in $out"
