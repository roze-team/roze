$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Invoke-Cargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $exitCode"
    }
}

Write-Host "Running Windows release preflight."
Write-Host "The authoritative release gate remains Linux/WSL-only."
Write-Host "rdkafka-cmake requires CMake and the Visual Studio C/C++ build tools."

Invoke-Cargo @("fmt", "--all", "--", "--check")
Invoke-Cargo @("clippy", "--workspace", "--exclude", "user-service", "--exclude", "roze-example", "--all-targets", "--", "-D", "warnings")
Invoke-Cargo @("test", "--workspace", "--exclude", "user-service", "--exclude", "roze-example")
Invoke-Cargo @("check", "--workspace", "--exclude", "user-service", "--exclude", "roze-example")
Invoke-Cargo @("check", "-p", "roze-kafka", "--no-default-features", "--features", "rdkafka-cmake")
Invoke-Cargo @("check", "-p", "roze-kafka", "--no-default-features", "--features", "rskafka")
Invoke-Cargo @("test", "-p", "rozectl", "--", "--skip", "postgres", "--skip", "mysql", "--skip", "mongo")
Invoke-Cargo @("test", "-p", "roze-gateway")
Invoke-Cargo @("test", "-p", "roze-config", "config_center")

Get-ChildItem -Path $root -Recurse -Filter "roze-service.yaml" -File |
    Where-Object {
        $_.FullName -notmatch '[\\/](\.git|target(?:-[^\\/]+)?)[\\/]'
    } |
    ForEach-Object {
        Invoke-Cargo @(
            "run", "--quiet", "-p", "rozectl", "--",
            "service", "sync", "--project", $_.DirectoryName, "--check"
        )
    }

Write-Host "Windows release preflight passed. Run 'bash scripts/release-gate.sh' on Linux/WSL before release."
