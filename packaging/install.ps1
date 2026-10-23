# StewardGet installer for Windows 10 and 11 (x64 and ARM64).
#
#   irm https://github.com/anand34577/stewardget/releases/latest/download/install.ps1 | iex
#
# Downloads StewardGet-Setup.exe, checks it against the release's SHA256SUMS.txt,
# installs it silently (no administrator rights needed) and opens StewardGet.
# A particular version: $env:STEWARDGET_VERSION = "0.2.0" before running.
# A mirror of the release files: $env:STEWARDGET_MIRROR = "https://mirror.example/stewardget/v0.2.0".
# Uninstall from Settings > Apps > Installed apps > StewardGet.

& {
    $ErrorActionPreference = 'Stop'
    # Invoke-WebRequest is many times faster without the progress bar.
    $ProgressPreference = 'SilentlyContinue'

    $repo = 'anand34577/stewardget'
    $appKey = '{8F4E2D1C-6B7A-4C3E-9A51-2D7B0E5F9C14}_is1'

    function Fail([string]$message) {
        Write-Host "error: $message" -ForegroundColor Red
        throw 'StewardGet was not installed.'
    }

    function InstallLocation {
        foreach ($root in 'HKCU:', 'HKLM:') {
            $key = "$root\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appKey"
            if (Test-Path $key) {
                $location = (Get-ItemProperty $key).InstallLocation
                if ($location) { return @{ Path = $location.TrimEnd('\'); Machine = ($root -eq 'HKLM:') } }
            }
        }
        return $null
    }

    if ([Environment]::OSVersion.Version.Major -lt 10 -or -not [Environment]::Is64BitOperatingSystem) {
        Fail 'StewardGet needs 64-bit Windows 10 or 11.'
    }
    # Windows PowerShell 5.1 may not offer TLS 1.2 by default.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $version = "$env:STEWARDGET_VERSION".Trim().TrimStart('v')
    $base = if ($env:STEWARDGET_MIRROR) { "$env:STEWARDGET_MIRROR".TrimEnd('/') } elseif ($version) { "https://github.com/$repo/releases/download/v$version" } else { "https://github.com/$repo/releases/latest/download" }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("stewardget-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $setup = Join-Path $tmp 'StewardGet-Setup.exe'
        $sums = Join-Path $tmp 'SHA256SUMS.txt'
        Write-Host '==> Downloading StewardGet' -ForegroundColor Cyan
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.txt" -OutFile $sums
            Invoke-WebRequest -UseBasicParsing -Uri "$base/StewardGet-Setup.exe" -OutFile $setup
        } catch {
            Fail "could not download from GitHub ($($_.Exception.Message)). Check your connection$(if ($version) { " and that version $version exists" })."
        }

        $line = Get-Content $sums | Where-Object { $_ -match '^\s*([0-9a-fA-F]{64})\s+\*?StewardGet-Setup\.exe\s*$' } | Select-Object -First 1
        if (-not $line) { Fail 'StewardGet-Setup.exe is not listed in SHA256SUMS.txt.' }
        $expected = ($line.Trim() -split '\s+')[0].ToUpperInvariant()
        $actual = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToUpperInvariant()
        if ($expected -ne $actual) { Fail 'checksum mismatch: the download is damaged or was altered. Nothing was installed.' }

        # Let a running StewardGet save and quit; Setup would close it anyway. `--quit` works even
        # when the close button only minimises StewardGet; older versions get a close request.
        $running = Get-Process -Name 'stewardget-gui' -ErrorAction SilentlyContinue
        if ($running) {
            Write-Host '==> Closing StewardGet (it saves its progress first)' -ForegroundColor Cyan
            $current = $running | Where-Object { $_.Path } | Select-Object -First 1
            if ($current) {
                Start-Process -FilePath $current.Path -ArgumentList '--quit' -Wait -WindowStyle Hidden -ErrorAction SilentlyContinue
            }
            $running = Get-Process -Name 'stewardget-gui' -ErrorAction SilentlyContinue
            $running | ForEach-Object { [void]$_.CloseMainWindow() }
            $running | ForEach-Object { [void]$_.WaitForExit(10000) }
        }

        # Update an all-users install in place (asks for administrator rights); otherwise install for this user.
        $previous = InstallLocation
        $scope = if ($previous -and $previous.Machine) { '/ALLUSERS' } else { '/CURRENTUSER' }
        Write-Host '==> Installing' -ForegroundColor Cyan
        $process = Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', $scope -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            Fail "Setup stopped with exit code $($process.ExitCode). Try running StewardGet-Setup.exe from the release page: https://github.com/$repo/releases/latest"
        }

        $installed = InstallLocation
        if (-not $installed) { Fail "Setup finished but StewardGet was not found. Please report this: https://github.com/$repo/issues" }
        $exe = Join-Path $installed.Path 'stewardget-gui.exe'
        # Make `stewardget` available in this window too (new windows get it from PATH).
        if (($env:Path -split ';') -notcontains $installed.Path) { $env:Path = "$env:Path;$($installed.Path)" }

        $about = & (Join-Path $installed.Path 'stewardget.exe') --version 2>$null
        Write-Host ''
        Write-Host "==> $about is installed" -ForegroundColor Green
        Write-Host '    Desktop app: StewardGet in the Start menu'
        Write-Host '    Terminal:    stewardget'
        Write-Host '    Uninstall:   Settings > Apps > Installed apps > StewardGet'
        Start-Process -FilePath $exe
    } finally {
        Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
    }
}
