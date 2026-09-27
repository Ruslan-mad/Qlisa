param(
    [Parameter(Mandatory = $true)]
    [string] $ProfileRoot,
    [string] $Executable = (Join-Path $PSScriptRoot '..\..\src-tauri\target\debug\qlisa.exe'),
    [string] $Ffmpeg = (Join-Path $env:LOCALAPPDATA 'Qlisa\runtime\ffmpeg.exe')
)

$ErrorActionPreference = 'Stop'
$ProfileRoot = [System.IO.Path]::GetFullPath($ProfileRoot)
$Executable = [System.IO.Path]::GetFullPath($Executable)
$Ffmpeg = [System.IO.Path]::GetFullPath($Ffmpeg)
$project = Join-Path $ProfileRoot 'Show\AudioUnderrun.qlisa'
$roaming = Join-Path $ProfileRoot 'AppData\Roaming'

foreach ($path in @($Executable, $Ffmpeg, $project, (Join-Path $roaming 'Inkue\audio.json'), (Join-Path $roaming 'Inkue\preferences.json'), (Join-Path $roaming 'Inkue\osc.json'))) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required test-profile file is missing: $path"
    }
}

# Restrict the alternate profile to this child process. LOCALAPPDATA remains
# the current Windows value so runtime caches and WebView data stay unchanged.
$oldAppData = $env:APPDATA
$oldFfmpeg = $env:QLISA_FFMPEG_PATH
try {
    $env:APPDATA = $roaming
    $env:QLISA_FFMPEG_PATH = $Ffmpeg
    $process = Start-Process -FilePath $Executable -ArgumentList @('"' + $project + '"') -WorkingDirectory (Split-Path -Parent $Executable) -PassThru
} finally {
    $env:APPDATA = $oldAppData
    if ($null -eq $oldFfmpeg) { Remove-Item Env:\QLISA_FFMPEG_PATH -ErrorAction SilentlyContinue }
    else { $env:QLISA_FFMPEG_PATH = $oldFfmpeg }
}

Write-Output "Started debug app PID $($process.Id)"
Write-Output "Project: $project"
Write-Output "APPDATA: $roaming"
Write-Output "LOCALAPPDATA: $env:LOCALAPPDATA"
