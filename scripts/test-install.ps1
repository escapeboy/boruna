# Offline test of install.ps1: builds a fake release from target\debug and checks that a good
# archive installs, a tampered archive installs nothing, and a missing target fails.
# Run after `cargo build --workspace`, under Windows PowerShell 5.1 and PowerShell 7.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path ([IO.Path]::GetTempPath()) ("boruna-install-test-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
$binaries = @('boruna', 'boruna-mcp', 'boruna-pkg', 'boruna-orch')

function Fail([string]$msg) { throw "FAIL $msg" }

# Runs install.ps1 the way users do (piped into iex) in a child process, with the given
# environment, and returns the exit code and the combined output.
function Invoke-Installer([hashtable]$envVars) {
    $set = ($envVars.GetEnumerator() | ForEach-Object { "`$env:$($_.Key) = '$($_.Value)';" }) -join ' '
    $cmd = "$set Get-Content -Raw -LiteralPath '$root\install.ps1' | Invoke-Expression"
    $exe = (Get-Process -Id $PID).Path
    $out = & $exe -NoProfile -NonInteractive -Command $cmd 2>&1 | Out-String
    return @{ Code = $LASTEXITCODE; Out = $out }
}

function New-Release([string]$dir) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $sums = @()
    foreach ($t in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
        $name = "boruna-0.0.0-test-$t"
        $stage = Join-Path $work "stage\$name"
        New-Item -ItemType Directory -Force -Path $stage | Out-Null
        foreach ($b in $binaries) { Copy-Item (Join-Path $root "target\debug\$b.exe") $stage }
        $zip = Join-Path $dir "$name.zip"
        Compress-Archive -Path $stage -DestinationPath $zip
        $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        # Same "<hash> *<file>" form sha256sum writes on the Windows release builders.
        $sums += "$hash *$name.zip"
    }
    Set-Content -Path (Join-Path $dir 'SHA256SUMS') -Value $sums
}

try {
    New-Release (Join-Path $work 'good')
    $r = Invoke-Installer @{ BORUNA_DOWNLOAD_BASE = (Join-Path $work 'good'); BORUNA_INSTALL_DIR = (Join-Path $work 'bin'); BORUNA_NO_MODIFY_PATH = '1' }
    if ($r.Code -ne 0) { Write-Host $r.Out; Fail 'good release did not install' }
    foreach ($b in $binaries) {
        if (-not (Test-Path (Join-Path $work "bin\$b.exe"))) { Fail "$b.exe was not installed" }
    }
    $v = & (Join-Path $work 'bin\boruna.exe') --version
    if ($v -notmatch '^boruna ') { Fail "installed boruna does not run: $v" }
    Write-Host 'ok   good release installs all four binaries'

    Copy-Item -Recurse (Join-Path $work 'good') (Join-Path $work 'tampered')
    Get-ChildItem (Join-Path $work 'tampered') -Filter *.zip | ForEach-Object { Add-Content -LiteralPath $_.FullName -Value 'x' }
    $r = Invoke-Installer @{ BORUNA_DOWNLOAD_BASE = (Join-Path $work 'tampered'); BORUNA_INSTALL_DIR = (Join-Path $work 'bin2'); BORUNA_NO_MODIFY_PATH = '1' }
    if ($r.Code -eq 0) { Fail 'tampered archive was accepted' }
    if ($r.Out -notmatch 'checksum mismatch') { Write-Host $r.Out; Fail 'tampered archive failed for the wrong reason' }
    if (Test-Path (Join-Path $work 'bin2')) { Fail 'tampered archive left files behind' }
    Write-Host 'ok   tampered archive is refused and nothing is installed'

    New-Item -ItemType Directory -Path (Join-Path $work 'empty') | Out-Null
    Set-Content -Path (Join-Path $work 'empty\SHA256SUMS') -Value '0000 *boruna-0.0.0-test-sparc-unknown-none.zip'
    $r = Invoke-Installer @{ BORUNA_DOWNLOAD_BASE = (Join-Path $work 'empty'); BORUNA_INSTALL_DIR = (Join-Path $work 'bin3'); BORUNA_NO_MODIFY_PATH = '1' }
    if ($r.Code -eq 0) { Fail 'release without this platform was accepted' }
    if ($r.Out -notmatch 'has no archive for') { Write-Host $r.Out; Fail 'missing platform failed for the wrong reason' }
    Write-Host 'ok   release without this platform fails with a clear message'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
