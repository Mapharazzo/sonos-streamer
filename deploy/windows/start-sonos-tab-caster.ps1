param(
    [string]$IpAddress = "",
    [string]$ManagedConfig = ""
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
$CompanionArguments = @("--browser-source")
if ($ManagedConfig) {
    $CompanionArguments = @("--managed", "--managed-config", $ManagedConfig)
} elseif ($IpAddress) {
    $CompanionArguments += @("--ip_address", $IpAddress)
}

"Starting Sonos Tab Caster; log: $LogPath" | Tee-Object -FilePath $LogPath
$SetupErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    & $Executable @CompanionArguments 2>&1 |
        ForEach-Object { $_.ToString() } |
        Tee-Object -FilePath $LogPath -Append
    $ExitCode = $LASTEXITCODE
}
finally {
    $ErrorActionPreference = $SetupErrorActionPreference
}
"Companion exited with code $ExitCode" | Tee-Object -FilePath $LogPath -Append
exit $ExitCode
