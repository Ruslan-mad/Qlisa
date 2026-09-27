param(
    [string] $AudioCueNumber = "2",
    [string] $SecondAudioCueNumber = "3",
    [string] $VideoCueNumber = "1",
    [string] $HostName = "127.0.0.1",
    [int] $Port = 53001,
    [int] $Minutes = 30,
    [int] $CycleSeconds = 20,
    [int] $VideoRestartSeconds = 50
)

$ErrorActionPreference = "Stop"
$client = [System.Net.Sockets.UdpClient]::new()
$endpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Parse($HostName), $Port)

function ConvertTo-OscString([string] $Value) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Value + [char]0)
    $paddedLength = [int]([Math]::Ceiling($bytes.Length / 4.0) * 4)
    $result = [byte[]]::new($paddedLength)
    [Array]::Copy($bytes, $result, $bytes.Length)
    return ,$result
}

function Send-Osc([string] $Address, [Nullable[float]] $Value = $null) {
    $parts = [System.Collections.Generic.List[byte]]::new()
    $parts.AddRange((ConvertTo-OscString $Address))
    if ($null -eq $Value) {
        $parts.AddRange((ConvertTo-OscString ","))
    } else {
        $parts.AddRange((ConvertTo-OscString ",f"))
        $raw = [BitConverter]::GetBytes([single]$Value)
        if ([BitConverter]::IsLittleEndian) { [Array]::Reverse($raw) }
        $parts.AddRange($raw)
    }
    $packet = $parts.ToArray()
    [void]$client.Send($packet, $packet.Length, $endpoint)
    "{0:o} {1}{2}" -f [DateTime]::UtcNow, $Address, $(if ($null -eq $Value) { "" } else { " $Value" }) |
        Add-Content -LiteralPath $script:LogPath
}

$script:LogPath = Join-Path (Get-Location) ("audio-underrun-repro-{0:yyyyMMdd-HHmmss}.log" -f [DateTime]::Now)
$deadline = [DateTime]::UtcNow.AddMinutes($Minutes)
$videoRestartAt = [DateTime]::UtcNow.AddSeconds($VideoRestartSeconds)
$audioCues = @($AudioCueNumber)
if ($SecondAudioCueNumber -and $SecondAudioCueNumber -ne $AudioCueNumber) {
    $audioCues += $SecondAudioCueNumber
}
$audioIndex = 0
$currentAudioCue = $audioCues[$audioIndex]

try {
    # Start the video loop, then issue the first Audio Cue GO immediately after
    # app/project startup to capture the cold publication/prebuffer window.
    if ($VideoCueNumber) { Send-Osc "/inkue/cue/$VideoCueNumber/go" }
    Send-Osc "/inkue/cue/$currentAudioCue/go"

    while ([DateTime]::UtcNow -lt $deadline) {
        $cycleStarted = [DateTime]::UtcNow
        Start-Sleep -Seconds ([Math]::Max(3, [int]($CycleSeconds * 0.40)))
        Send-Osc "/inkue/pause"
        Start-Sleep -Seconds 1
        Send-Osc "/inkue/cue/$currentAudioCue/seek" 4.0
        Send-Osc "/inkue/resume"
        Start-Sleep -Seconds ([Math]::Max(1, [int]($CycleSeconds * 0.25)))

        Send-Osc "/inkue/cue/$currentAudioCue/stop"
        if ($audioCues.Count -gt 1) {
            $audioIndex = 1 - $audioIndex
            $currentAudioCue = $audioCues[$audioIndex]
        }
        Start-Sleep -Milliseconds 750
        Send-Osc "/inkue/cue/$currentAudioCue/go"

        if ($VideoCueNumber -and [DateTime]::UtcNow -ge $videoRestartAt) {
            Send-Osc "/inkue/cue/$VideoCueNumber/stop"
            Start-Sleep -Seconds 1
            Send-Osc "/inkue/cue/$VideoCueNumber/go"
            $videoRestartAt = [DateTime]::UtcNow.AddSeconds($VideoRestartSeconds)
        }

        $remaining = $CycleSeconds - ([DateTime]::UtcNow - $cycleStarted).TotalSeconds
        if ($remaining -gt 0) { Start-Sleep -Milliseconds ([int]($remaining * 1000)) }
    }
} finally {
    foreach ($cue in $audioCues) {
        try { Send-Osc "/inkue/cue/$cue/stop" } catch { }
    }
    if ($VideoCueNumber) { try { Send-Osc "/inkue/cue/$VideoCueNumber/stop" } catch { } }
    $client.Dispose()
}

Write-Output "OSC run log: $script:LogPath"
