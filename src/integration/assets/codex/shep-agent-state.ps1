# installed by shep
# managed by shep; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# SHEP_INTEGRATION_ID=codex
# SHEP_INTEGRATION_VERSION=6

param([string]$Action = "")

if ($Action -ne "session") { exit 0 }
if ($env:SHEP_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:SHEP_PANE_ID)) { exit 0 }

$inputText = [Console]::In.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

if ($payload.hook_event_name -and $payload.hook_event_name -ne "SessionStart") { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
try {
    $args = @(
        "pane",
        "report-agent-session",
        $env:SHEP_PANE_ID,
        "--source",
        "shep:codex",
        "--agent",
        "codex",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$sessionId"
    )
    if ($payload.hook_event_name -eq "SessionStart" -and $payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
        $args += @("--session-start-source", "$($payload.source)")
    }
    & shep @args 2>$null | Out-Null
} catch {
}
