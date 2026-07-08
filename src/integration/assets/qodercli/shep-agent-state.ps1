# installed by shep
# managed by shep; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# SHEP_INTEGRATION_ID=qodercli
# SHEP_INTEGRATION_VERSION=2

param([string]$Action = "")

if ($Action -ne "session") { exit 0 }
if ($env:SHEP_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:SHEP_PANE_ID)) { exit 0 }

$inputText = [Console]::In.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    $payload = $null
}

if ($null -eq $payload -or [string]::IsNullOrWhiteSpace($payload.session_id)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
try {
    & shep pane report-agent-session $env:SHEP_PANE_ID --source shep:qodercli --agent qodercli --agent-session-id $payload.session_id --seq $seq 2>$null | Out-Null
} catch {
}
