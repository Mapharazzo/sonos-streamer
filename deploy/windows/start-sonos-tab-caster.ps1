param(
    [string]$IpAddress = "192.168.69.111"
)

$ErrorActionPreference = "Stop"
$InstallRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Executable = Join-Path $InstallRoot "bin\swyh-rs-cli.exe"
$LogDirectory = Join-Path $InstallRoot "logs"

if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Companion executable not found: $Executable"
}

New-Item -ItemType Directory -Path $LogDirectory -Force | Out-Null
$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$LogPath = Join-Path $LogDirectory "companion-$Timestamp.log"

"Starting Sonos Tab Caster on $IpAddress; log: $LogPath" | Tee-Object -FilePath $LogPath
& $Executable --browser-source --ip_address $IpAddress 2>&1 |
    Tee-Object -FilePath $LogPath -Append
$ExitCode = $LASTEXITCODE
"Companion exited with code $ExitCode" | Tee-Object -FilePath $LogPath -Append
exit $ExitCode
