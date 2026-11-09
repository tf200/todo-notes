<#
.SYNOPSIS
  Builds, packs and publishes a release of Todo & Notes for Windows and Linux.

.DESCRIPTION
  Everything runs on this machine: the Windows build here, the Linux build in WSL, Velopack's
  `vpk` to pack both, and the upload to GitHub Releases, where installed apps look for updates.

    .\release.ps1 0.2.0            publish v0.2.0 (sets the version, commits, tags, uploads)
    .\release.ps1 0.2.0 -Local     build and pack into .\releases only: nothing is committed,
                                   tagged or uploaded (for trying an update before shipping it)

  macOS is released from a Mac afterwards with ./release-mac.sh <version>, into the same release.

  Needs: Visual Studio 2022 (C++ tools), WSL with Rust, `vpk` (dotnet tool install -g vpk), and a
  GitHub token: $env:GITHUB_TOKEN, or `gh auth login` inside WSL.
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$Local
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$Repo = 'https://github.com/tf200/todo-notes'
$PackId = 'TodoNotes'
$Title = 'Todo & Notes'
# The installed name (shortcuts, Apps & features). No "&": vpk writes it unescaped into XML,
# and escaped elsewhere, so it would show up as "&amp;".
$PackTitle = 'Todo and Notes'
$Authors = 'Taha'
$Tag = "v$Version"
$Out = Join-Path $PSScriptRoot 'releases'
$Stage = Join-Path $PSScriptRoot 'target\release-stage'

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Fail($text) {
    Write-Host "error: $text" -ForegroundColor Red
    Restore
    exit 1
}
# A local run (or a failed one) leaves Cargo.toml and Cargo.lock as they were.
$saved = $null
function Restore {
    if ($saved -and ($Local -or -not $script:committed)) {
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'Cargo.toml'), $saved.toml)
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'Cargo.lock'), $saved.lock)
    }
}
$script:committed = $false
# Runs a native command and stops the script when it fails.
function Run([string]$exe, [string[]]$arguments) {
    & $exe @arguments
    if ($LASTEXITCODE -ne 0) { Fail "$exe $($arguments[0]) failed (exit $LASTEXITCODE)" }
}
# WSL paths for this repo and the staging folder.
function WslPath($p) { (& wsl.exe wslpath -a ($p -replace '\\', '/')).Trim() }

if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "version must look like 1.2.3, got '$Version'" }
if (-not (Get-Command vpk -ErrorAction SilentlyContinue)) {
    $env:Path += ";$env:USERPROFILE\.dotnet\tools"
    if (-not (Get-Command vpk -ErrorAction SilentlyContinue)) { Fail "vpk not found: dotnet tool install -g vpk" }
}

if (-not $Local) {
    if (git status --porcelain) { Fail 'uncommitted changes: commit or stash them first' }
    if ((git branch --show-current) -ne 'main') { Fail 'release from main' }
    git rev-parse -q --verify "refs/tags/$Tag" | Out-Null
    if ($LASTEXITCODE -eq 0) { Fail "tag $Tag already exists" }
    $Token = $env:GITHUB_TOKEN
    if (-not $Token) { $Token = (& wsl.exe -e bash -lc 'gh auth token' 2>$null | Out-String).Trim() }
    if (-not $Token) { Fail 'no GitHub token: set $env:GITHUB_TOKEN or run gh auth login in WSL' }
}

# The version lives in Cargo.toml, so the app and its package agree. A local run puts the
# files back as they were afterwards (the build bumps Cargo.lock's copy of the version too).
Step "Version $Version"
$script:saved = @{ toml = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Cargo.toml')); lock = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Cargo.lock')) }
$saved = $script:saved
$cargo = $saved.toml
$cargo = [regex]::Replace($cargo, '(?m)^version = "[^"]*"', "version = `"$Version`"", 1)
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'Cargo.toml'), $cargo)

Step 'Building for Windows'
$vcvars = Get-ChildItem 'C:\Program Files\Microsoft Visual Studio\2022\*\VC\Auxiliary\Build\vcvars64.bat' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $vcvars) { Fail 'Visual Studio 2022 C++ tools not found (vcvars64.bat)' }
cmd /c "call `"$($vcvars.FullName)`" >nul && cargo build --release"
if ($LASTEXITCODE -ne 0) { Fail 'Windows build failed' }

Step 'Building for Linux (WSL)'
$wslRepo = WslPath $PSScriptRoot
# The linker wants libxkbcommon.so; desktops ship libxkbcommon.so.0, so link against that.
$linux = @"
set -e
mkdir -p ~/.cache/todo-notes/linklibs
ln -sf /usr/lib/x86_64-linux-gnu/libxkbcommon.so.0 ~/.cache/todo-notes/linklibs/libxkbcommon.so
cd '$wslRepo'
RUSTFLAGS="-L `$HOME/.cache/todo-notes/linklibs" CARGO_TARGET_DIR=`$HOME/.cache/todo-notes/target cargo build --release
"@
$linux = $linux -replace "`r", ''
Run wsl.exe @('-e', 'bash', '-lc', $linux)

Step 'Staging'
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$Stage\win", "$Stage\linux" | Out-Null
Copy-Item target\release\todo-notes.exe "$Stage\win\"
$wslStage = WslPath "$Stage\linux"
Run wsl.exe @('-e', 'bash', '-lc', "cp ~/.cache/todo-notes/target/release/todo-notes '$wslStage/' && chmod +x '$wslStage/todo-notes'")

$notes = Join-Path $Stage 'notes.md'
"# $Title $Version" | Set-Content -Encoding utf8 $notes

foreach ($channel in 'win', 'linux') {
    $dir = Join-Path $Out $channel
    New-Item -ItemType Directory -Force $dir | Out-Null
    if (-not $Local) {
        # The previous release, so this one can ship as a small delta.
        Step "Fetching the last $channel release"
        & vpk download github --repoUrl $Repo --token $Token --channel $channel -o $dir
    }
}

Step 'Packing for Windows'
Run vpk @('pack', '-u', $PackId, '-v', $Version, '-p', "$Stage\win", '-e', 'todo-notes.exe',
    '-o', "$Out\win", '--channel', 'win', '-i', 'assets\icon\icon.ico',
    '--packTitle', $PackTitle, '--packAuthors', $Authors, '--releaseNotes', $notes,
    '--shortcuts', 'StartMenuRoot,Desktop')

Step 'Packing for Linux'
Run vpk @('[linux]', 'pack', '-u', $PackId, '-v', $Version, '-p', "$Stage\linux", '-e', 'todo-notes',
    '-o', "$Out\linux", '--channel', 'linux', '-r', 'linux-x64', '-i', 'assets\icon\icon.png',
    '--packTitle', $PackTitle, '--packAuthors', $Authors, '--releaseNotes', $notes)

if ($Local) {
    Restore
    Step "Packed into $Out (nothing committed or uploaded)"
    exit 0
}

Step "Committing and tagging $Tag"
Run git @('add', 'Cargo.toml', 'Cargo.lock')
Run git @('commit', '-q', '-m', "Release $Tag")
$script:committed = $true
Run git @('tag', '-a', $Tag, '-m', "$Title $Version")
Run git @('push', '-q', 'origin', 'main', $Tag)

Step 'Uploading to GitHub Releases'
Run vpk @('upload', 'github', '-o', "$Out\win", '--channel', 'win', '--repoUrl', $Repo, '--token', $Token,
    '--tag', $Tag, '--releaseName', "$Title $Version", '--publish')
Run vpk @('upload', 'github', '-o', "$Out\linux", '--channel', 'linux', '--repoUrl', $Repo, '--token', $Token,
    '--tag', $Tag, '--releaseName', "$Title $Version", '--publish', '--merge')

Step "Released ${Tag}: $Repo/releases/tag/$Tag"
