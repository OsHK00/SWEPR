param(
    [string]$Version = "1.0.0"
)

$ErrorActionPreference = "Stop"

if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version must look like 1.0.0 or 1.0.0-rc1."
}

$repoRoot = $PSScriptRoot
$buildTargetDirectory = Join-Path $repoRoot "target\release-package-build"
$outputDirectory = Join-Path $repoRoot "releases\v$Version"
$archivePath = Join-Path $outputDirectory "SWEPR-v$Version-windows-x64.zip"
$stageDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("swepr-release-" + [guid]::NewGuid().ToString("N"))

try {
    Push-Location $repoRoot
    try {
        & cargo build --release --target-dir $buildTargetDirectory
        if ($LASTEXITCODE -ne 0) {
            throw "Release build failed. Close SWEPR if it is running and try again."
        }
    }
    finally {
        Pop-Location
    }

    $executable = Join-Path $buildTargetDirectory "release\swepr.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Build succeeded, but the release executable was not found."
    }

    New-Item -ItemType Directory -Path $stageDirectory -Force | Out-Null
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $stageDirectory "swepr.exe")
    Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination $stageDirectory
    Copy-Item -LiteralPath (Join-Path $repoRoot "languages") -Destination $stageDirectory -Recurse

    Compress-Archive -Path (Join-Path $stageDirectory "*") -DestinationPath $archivePath -Force
    Write-Host "Release package created: $archivePath"
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
