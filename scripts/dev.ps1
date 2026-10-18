param(
    [string[]]$WowAddOnsPath
)

$ErrorActionPreference = 'Stop'

$addonName = 'TranslateProbe'
$projectRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$source = Join-Path $projectRoot "addon\$addonName"

# Personal machine paths never live in source control. Provide yours via:
#  - the -WowAddOnsPath parameter (one or more paths),
#  - the TPROBE_WOW_ADDONS_PATH environment variable (';'-separated), or
#  - a local scripts\dev.local.ps1 file (gitignored) that sets $WowAddOnsPaths
#    (array) or $WowAddOnsPath (single string).
$targets = @()
if ($WowAddOnsPath) { $targets = @($WowAddOnsPath) }

if (-not $targets) {
    $localOverride = Join-Path $PSScriptRoot 'dev.local.ps1'
    if (Test-Path -LiteralPath $localOverride) {
        . $localOverride
        if ($WowAddOnsPaths) { $targets = @($WowAddOnsPaths) }
        elseif ($WowAddOnsPath) { $targets = @($WowAddOnsPath) }
    }
}

if (-not $targets -and $env:TPROBE_WOW_ADDONS_PATH) {
    $targets = $env:TPROBE_WOW_ADDONS_PATH -split ';' | Where-Object { $_ }
}

# WoW Forever runs from the _classic_beta_ folder during its beta.
if (-not $targets) {
    $targets = @(
        "${env:ProgramFiles(x86)}\World of Warcraft\_classic_beta_\Interface\AddOns",
        "$env:ProgramFiles\World of Warcraft\_classic_beta_\Interface\AddOns"
    ) | Where-Object { Test-Path -LiteralPath $_ }
}

if (-not $targets) {
    throw "No WoW AddOns folder found. Pass -WowAddOnsPath, set TPROBE_WOW_ADDONS_PATH, or create scripts\dev.local.ps1 setting `$WowAddOnsPaths."
}

if (-not (Test-Path -LiteralPath (Join-Path $source "$addonName.toc"))) {
    throw "TOC file not found in addon source: $source"
}

# Only ever deploy into a real WoW AddOns folder, whatever the flavour.
$flavourPattern = 'World of Warcraft\\_[^\\]+_\\Interface\\AddOns$'
$deployed = 0

foreach ($path in $targets) {
    if (-not $path) { continue }
    if (-not (Test-Path -LiteralPath $path)) {
        New-Item -ItemType Directory -Force -Path $path | Out-Null
    }

    $resolved = (Resolve-Path -LiteralPath $path).Path.TrimEnd('\')
    if ($resolved -notmatch $flavourPattern) {
        throw "Refusing to deploy outside a WoW AddOns folder: $resolved"
    }

    $destination = Join-Path $resolved $addonName
    New-Item -ItemType Directory -Force -Path $destination | Out-Null

    robocopy $source $destination /MIR /NFL /NDL /NJH /NJS /NP /XF *.zip | Out-Host
    $code = $LASTEXITCODE
    if ($code -gt 7) {
        throw "robocopy failed with exit code $code (target: $destination)"
    }

    Write-Host "DEV deploy complete -> $destination"

    $deployed++
}

Write-Host "Deployed to $deployed folder(s). Type /reload in game to load the changes."
exit 0
