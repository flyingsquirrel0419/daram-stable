Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repo = if ($env:DR_REPO) { $env:DR_REPO } else { 'flyingsquirrel0419/daram-stable' }
$RequestedVersion = if ($env:DR_VERSION) { $env:DR_VERSION } else { '' }
$InstallDir = if ($env:DR_INSTALL_DIR) { $env:DR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\Daram\bin' }

function Write-Log {
    param([string]$Message)
    Write-Host "[daram-install] $Message"
}

function Fail {
    param([string]$Message)
    throw "[daram-install] error: $Message"
}

function Normalize-Version {
    param([string]$Version)

    if ([string]::IsNullOrWhiteSpace($Version)) {
        return 'latest'
    }

    if ($Version.StartsWith('v')) {
        return $Version
    }

    return "v$Version"
}

function Resolve-Tag {
    param([string]$Version)

    $normalized = Normalize-Version $Version
    if ($normalized -ne 'latest') {
        return $normalized
    }

    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    if (-not $latest.tag_name) {
        Fail 'Failed to resolve latest release tag'
    }

    return [string]$latest.tag_name
}

function Asset-Version {
    param([string]$Tag)

    return $Tag.TrimStart('v')
}

function Download-Url {
    param(
        [string]$Tag,
        [string]$Asset
    )

    if ($Tag -eq 'latest') {
        return "https://github.com/$Repo/releases/latest/download/$Asset"
    }

    return "https://github.com/$Repo/releases/download/$Tag/$Asset"
}

function Detect-Target {
    if (-not [Environment]::Is64BitOperatingSystem) {
        Fail 'Only 64-bit Windows is supported'
    }

    return 'x86_64-pc-windows-msvc'
}

function Verify-Checksum {
    param(
        [string]$ChecksumFile,
        [string]$AssetName,
        [string]$AssetPath
    )

    $expectedLine = Get-Content $ChecksumFile | Where-Object { $_ -match "  $([regex]::Escape($AssetName))$" } | Select-Object -First 1
    if (-not $expectedLine) {
        Fail "Checksum entry not found for $AssetName"
    }

    $expected = ($expectedLine -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Path $AssetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "Checksum mismatch for $AssetName"
    }
}

$tag = Resolve-Tag $RequestedVersion
$version = Asset-Version $tag
$target = Detect-Target
$asset = "dr-$version-$target.zip"

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("daram-install-" + [System.Guid]::NewGuid().ToString('n'))
$extractDir = Join-Path $tmpRoot 'extract'
$assetPath = Join-Path $tmpRoot $asset
$checksumPath = Join-Path $tmpRoot 'SHA256SUMS'

New-Item -ItemType Directory -Force -Path $extractDir | Out-Null

try {
    Write-Log "Downloading $asset"
    Invoke-WebRequest -Uri (Download-Url $tag $asset) -OutFile $assetPath

    Write-Log 'Downloading SHA256SUMS'
    Invoke-WebRequest -Uri (Download-Url $tag 'SHA256SUMS') -OutFile $checksumPath

    Verify-Checksum -ChecksumFile $checksumPath -AssetName $asset -AssetPath $assetPath

    Expand-Archive -Path $assetPath -DestinationPath $extractDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    $binaryPath = Join-Path $InstallDir 'dr.exe'
    Copy-Item -Path (Join-Path $extractDir 'dr.exe') -Destination $binaryPath -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) {
        $userPath = ''
    }

    $pathEntries = $userPath -split ';' | Where-Object { $_ }
    $pathChanged = $false
    if ($pathEntries -notcontains $InstallDir) {
        $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) {
            $InstallDir
        } else {
            "$userPath;$InstallDir"
        }

        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $pathChanged = $true
    }

    $sessionEntries = $env:Path -split ';' | Where-Object { $_ }
    if ($sessionEntries -notcontains $InstallDir) {
        $env:Path = if ([string]::IsNullOrWhiteSpace($env:Path)) {
            $InstallDir
        } else {
            "$InstallDir;$env:Path"
        }
    }

    Write-Log "Installed dr to $binaryPath"
    $versionOutput = & $binaryPath --version
    if ($versionOutput) {
        Write-Log "Verified binary: $versionOutput"
    }

    if ($pathChanged) {
        Write-Log "Added $InstallDir to your user PATH and updated the current PowerShell session"
    }
    Write-Log "Run 'dr --version' to verify the installation"
    Write-Log 'Rust is not required to use dr; native builds may require a system C compiler'
}
finally {
    if (Test-Path $tmpRoot) {
        Remove-Item -Path $tmpRoot -Recurse -Force
    }
}
