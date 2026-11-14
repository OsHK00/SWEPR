param(
    # Por defecto, la versión de Cargo.toml (la misma que llevan el manifiesto y el .exe).
    [string]$Version = "",
    # Además del instalador, crear un ZIP portable.
    [switch]$Portable
)

$ErrorActionPreference = "Stop"

$cargoVersion = (Select-String -Path (Join-Path $PSScriptRoot "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Version) {
    $Version = $cargoVersion
}
elseif ($Version -ne $cargoVersion) {
    throw "Version $Version does not match Cargo.toml ($cargoVersion). Update Cargo.toml first."
}

if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version must look like 1.0.0 or 1.0.0-rc1."
}

# Busca el compilador de Inno Setup 6 (variable ISCC, PATH, registro o rutas habituales).
function Find-InnoSetupCompiler {
    if ($env:ISCC -and (Test-Path -LiteralPath $env:ISCC -PathType Leaf)) { return $env:ISCC }
    $inPath = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($inPath) { return $inPath.Source }
    $uninstallKeys = @(
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
    )
    foreach ($key in $uninstallKeys) {
        $location = (Get-ItemProperty -Path $key -Name InstallLocation -ErrorAction SilentlyContinue).InstallLocation
        if ($location) {
            $candidate = Join-Path $location "ISCC.exe"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    foreach ($candidate in @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

$repoRoot = $PSScriptRoot
$buildTargetDirectory = Join-Path $repoRoot "target\release-package-build"
$outputDirectory = Join-Path $repoRoot "releases\v$Version"
$archivePath = Join-Path $outputDirectory "SWEPR-v$Version-windows-x64.zip"
$stageDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("swepr-release-" + [guid]::NewGuid().ToString("N"))

$iscc = Find-InnoSetupCompiler
if (-not $iscc) {
    throw "Inno Setup 6 was not found. Install it (winget install JRSoftware.InnoSetup) or set the ISCC environment variable to ISCC.exe."
}

try {
    Push-Location $repoRoot
    try {
        # Carpeta de compilación propia: así no choca con un swepr.exe en ejecución.
        & cargo build --release --target-dir $buildTargetDirectory
        if ($LASTEXITCODE -ne 0) {
            throw "Release build failed."
        }
    }
    finally {
        Pop-Location
    }

    $executable = Join-Path $buildTargetDirectory "release\swepr.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Build succeeded, but the release executable was not found."
    }

    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null

    # Instalador: instala y actualiza (el mismo setup sirve para ambas cosas).
    & $iscc "/Qp" "/DAppVersion=$Version" "/DSourceExe=$executable" "/DOutputDir=$outputDirectory" (Join-Path $repoRoot "installer\swepr.iss")
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed to build the installer."
    }
    Write-Host "Installer created: $(Join-Path $outputDirectory "SWEPR-v$Version-setup.exe")"

    if ($Portable) {
        New-Item -ItemType Directory -Path $stageDirectory -Force | Out-Null
        Copy-Item -LiteralPath $executable -Destination (Join-Path $stageDirectory "swepr.exe")
        Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination $stageDirectory
        Copy-Item -LiteralPath (Join-Path $repoRoot "languages") -Destination $stageDirectory -Recurse
        Compress-Archive -Path (Join-Path $stageDirectory "*") -DestinationPath $archivePath -Force
        Write-Host "Portable package created: $archivePath"
    }
}
finally {
    if (Test-Path -LiteralPath $stageDirectory) {
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
        $stageFullPath = [System.IO.Path]::GetFullPath($stageDirectory)
        $tempPrefix = $tempRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
        if ($stageFullPath.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $stageFullPath).StartsWith("swepr-release-")) {
            Remove-Item -LiteralPath $stageFullPath -Recurse -Force
        }
    }
}
