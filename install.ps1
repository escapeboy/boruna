# Install the Boruna binaries (boruna, boruna-mcp, boruna-pkg, boruna-orch) on Windows from a GitHub release.
#
#   irm https://raw.githubusercontent.com/escapeboy/boruna/master/install.ps1 | iex
#
# Windows x64 and Windows on Arm, PowerShell 5.1 or 7+. On Linux and macOS use install.sh.
# The archive is checked against the release's SHA256SUMS before anything is installed.
#
# Environment:
#   BORUNA_VERSION          release tag to install, e.g. v3.4.0 (default: latest)
#   BORUNA_INSTALL_DIR      where the binaries go (default: %LOCALAPPDATA%\Programs\boruna\bin)
#   BORUNA_DOWNLOAD_BASE    URL or local folder holding SHA256SUMS and the archives (default: the GitHub release)
#   BORUNA_NO_MODIFY_PATH   set to 1 to leave the user PATH alone

# Runs under `iex`, so errors are thrown, never `exit`: exit would close the user's shell.
function Install-Boruna {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'escapeboy/boruna'
    $binaries = @('boruna', 'boruna-mcp', 'boruna-pkg', 'boruna-orch')

    # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub refuses.
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }

    $arch = $null
    try { $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch { }
    if (-not $arch) {
        # An x64 PowerShell on an Arm machine reports the emulated CPU here; PROCESSOR_ARCHITEW6432 has the real one.
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    switch ($arch) {
        { $_ -in 'X64', 'AMD64' } { $target = 'x86_64-pc-windows-msvc' }
        { $_ -in 'Arm64', 'ARM64' } { $target = 'aarch64-pc-windows-msvc' }
        default { throw "unsupported CPU architecture '$arch' (supported: x64, Arm64). Build from source: https://github.com/$repo" }
    }

    $version = if ($env:BORUNA_VERSION) { $env:BORUNA_VERSION } else { 'latest' }
    $installDir = if ($env:BORUNA_INSTALL_DIR) { $env:BORUNA_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\boruna\bin' }
    if ($env:BORUNA_DOWNLOAD_BASE) { $base = $env:BORUNA_DOWNLOAD_BASE }
    elseif ($version -eq 'latest') { $base = "https://github.com/$repo/releases/latest/download" }
    else { $base = "https://github.com/$repo/releases/download/$version" }

    function Get-ReleaseFile([string]$name, [string]$dest) {
        if ($base -match '^https?://') {
            try { Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $dest }
            catch { throw "download failed: $base/$name ($($_.Exception.Message))" }
        } else {
            Copy-Item -LiteralPath (Join-Path $base $name) -Destination $dest
        }
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("boruna-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Installing Boruna ($version) for $target"
        Get-ReleaseFile 'SHA256SUMS' (Join-Path $tmp 'SHA256SUMS')

        # Lines look like "<hash>  <file>" or "<hash> *<file>" (binary mode).
        $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "-$([regex]::Escape($target))\.zip$" } | Select-Object -First 1
        if (-not $line) { throw "release $version has no archive for $target" }
        $parts = $line -split '\s+', 2
        $expected = $parts[0].ToLower()
        $zip = $parts[1].TrimStart('*')

        $zipPath = Join-Path $tmp $zip
        Get-ReleaseFile $zip $zipPath
        $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) {
            throw "checksum mismatch for $zip (expected $expected, got $actual); nothing was installed"
        }
        Write-Host "Checksum OK: $zip"

        Expand-Archive -LiteralPath $zipPath -DestinationPath $tmp
        $src = Join-Path $tmp ([IO.Path]::GetFileNameWithoutExtension($zip))
        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        foreach ($bin in $binaries) {
            $file = Join-Path $src "$bin.exe"
            if (-not (Test-Path -LiteralPath $file)) { throw "archive is missing $bin.exe" }
            Copy-Item -LiteralPath $file -Destination (Join-Path $installDir "$bin.exe") -Force
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Installed to ${installDir}: $($binaries -join ', ')"
    & (Join-Path $installDir 'boruna.exe') --version

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $onPath = ($userPath -split ';') -contains $installDir
    if (-not $onPath) {
        if ($env:BORUNA_NO_MODIFY_PATH -eq '1') {
            Write-Host "Note: $installDir is not on your PATH."
        } else {
            $newPath = if ($userPath) { "$installDir;$userPath" } else { $installDir }
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            Write-Host "Added $installDir to your user PATH. Open a new terminal to use 'boruna'."
        }
    }
}

Install-Boruna
