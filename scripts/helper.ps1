<#
.SYNOPSIS
Builds, tests or runs Translate Companion (helper\app, WoWTranslateCompanion.exe).

.DESCRIPTION
-Build       cargo build --offline --release of helper\app, then checks that the exe is under 3 MB.
-Test        cargo test --offline in every helper crate that exists (bridge, engine, app, testkit),
             one after the other.
-Run         starts the release exe (built first when missing) with --runtime when a translator
             runtime folder is known. Extra arguments after the switches go to the exe.
-Diag        like -Run, plus --diag and --diag-json %LOCALAPPDATA%\TranslateProbe\logs\diag-<time>.jsonl.
-RuntimeDir  llama.cpp runtime folder (holds llama-server.exe).

Personal machine paths never live in source control. The runtime folder comes from -RuntimeDir,
else $LlamaRuntimeDir in the gitignored scripts\dev.local.ps1, else the TPROBE_LLAMA_RUNTIME
environment variable. Without one, Translate Companion searches its default places (see docs\HELPER.md).

The exe was TranslateProbeHelper.exe until 2026-10-19. Both take the same single-instance mutex,
so exit an old TranslateProbeHelper.exe (tray icon, Exit) before starting the new one.

.EXAMPLE
scripts\helper.ps1 -Build -Test

.EXAMPLE
scripts\helper.ps1 -Run -Diag

.EXAMPLE
scripts\helper.ps1 -Run --capture gdi
#>
param(
    [switch]$Build,
    [switch]$Test,
    [switch]$Run,
    [switch]$Diag,
    [string]$RuntimeDir,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$HelperArgs
)

$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$appDir = Join-Path $projectRoot 'helper\app'
$exe = Join-Path $appDir 'target\release\WoWTranslateCompanion.exe'

if (-not ($Build -or $Test -or $Run -or $Diag)) {
    Write-Host 'Usage: scripts\helper.ps1 [-Build] [-Test] [-Run] [-Diag] [-RuntimeDir DIR] [helper arguments]'
    Write-Host 'See: Get-Help scripts\helper.ps1 -Detailed'
    exit 1
}
if ($Diag) { $Run = $true }

if (-not $RuntimeDir) {
    $localOverride = Join-Path $PSScriptRoot 'dev.local.ps1'
    if (Test-Path -LiteralPath $localOverride) {
        . $localOverride
        if ($LlamaRuntimeDir) { $RuntimeDir = $LlamaRuntimeDir }
    }
}
if (-not $RuntimeDir -and $env:TPROBE_LLAMA_RUNTIME) {
    $RuntimeDir = $env:TPROBE_LLAMA_RUNTIME
}

# Quotes one argument for the command-line parser of the exe (the MSVC / CommandLineToArgvW
# rules): inside quotes, backslashes are literal except before a quote, where each one is doubled,
# and an embedded quote becomes \". So 'C:\Program Files\rt\' arrives intact.
function ConvertTo-CommandLineArg {
    param([string]$Arg)
    if ($Arg -ne '' -and $Arg -notmatch '[\s"]') { return $Arg }
    $backslash = [char]92
    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.Append('"')
    $run = 0
    foreach ($ch in $Arg.ToCharArray()) {
        if ($ch -eq $backslash) {
            $run++
            continue
        }
        if ($ch -eq '"') {
            [void]$sb.Append($backslash, 2 * $run + 1).Append('"')
        }
        else {
            if ($run) { [void]$sb.Append($backslash, $run) }
            [void]$sb.Append($ch)
        }
        $run = 0
    }
    if ($run) { [void]$sb.Append($backslash, 2 * $run) }
    [void]$sb.Append('"')
    return $sb.ToString()
}

function Invoke-Cargo {
    param([string]$Dir, [string[]]$CargoArgs)
    Push-Location -LiteralPath $Dir
    try {
        & cargo @CargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw "cargo $($CargoArgs -join ' ') failed in $Dir (exit code $LASTEXITCODE)"
        }
    }
    finally {
        Pop-Location
    }
}

if ($Test) {
    # One cargo at a time; helper\.cargo\config.toml also limits each build to 3 jobs.
    # Every crate is tested even when an earlier one fails; the failures are listed at the end.
    $failed = @()
    foreach ($crate in @('bridge', 'engine', 'app', 'testkit')) {
        $dir = Join-Path $projectRoot "helper\$crate"
        if (Test-Path -LiteralPath (Join-Path $dir 'Cargo.toml')) {
            Write-Host "== cargo test --offline (helper\$crate)"
            try {
                Invoke-Cargo -Dir $dir -CargoArgs @('test', '--offline')
            }
            catch {
                $failed += "helper\$crate"
            }
        }
    }
    if ($failed) {
        throw "cargo test failed in: $($failed -join ', ')"
    }
}

if ($Build -or ($Run -and -not (Test-Path -LiteralPath $exe))) {
    Write-Host '== cargo build --offline --release (helper\app)'
    Invoke-Cargo -Dir $appDir -CargoArgs @('build', '--offline', '--release')
    $size = (Get-Item -LiteralPath $exe).Length
    Write-Host ('WoWTranslateCompanion.exe: {0:N0} bytes' -f $size)
    if ($size -ge 3MB) {
        throw 'WoWTranslateCompanion.exe must stay under 3 MB'
    }
}

if ($Run) {
    $argList = @()
    if ($RuntimeDir) {
        if (-not (Test-Path -LiteralPath (Join-Path $RuntimeDir 'llama-server.exe'))) {
            Write-Warning "No llama-server.exe in the runtime folder: $RuntimeDir"
        }
        $argList += @('--runtime', $RuntimeDir)
    }
    else {
        Write-Host 'No runtime folder given: Translate Companion searches its default places.'
    }
    if ($Diag) {
        $logDir = Join-Path $env:LOCALAPPDATA 'TranslateProbe\logs'
        New-Item -ItemType Directory -Force -Path $logDir | Out-Null
        $diagFile = Join-Path $logDir ('diag-{0}.jsonl' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
        $argList += @('--diag', '--diag-json', $diagFile)
        Write-Host "Diagnostics: $diagFile"
    }
    if ($HelperArgs) { $argList += $HelperArgs }
    # Start-Process joins the arguments with plain spaces: quote each one for the exe's parser.
    $quoted = $argList | ForEach-Object { ConvertTo-CommandLineArg $_ }
    $startArgs = @{ FilePath = $exe; PassThru = $true }
    if ($quoted) { $startArgs.ArgumentList = ($quoted -join ' ') }
    $proc = Start-Process @startArgs
    Write-Host "Translate Companion started (pid $($proc.Id)). Exit it from its tray icon."
}
exit 0
