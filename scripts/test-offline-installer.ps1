[CmdletBinding()]
param(
    [string]$ReleaseTag,
    [string]$ExpectedFullSha256,
    [string]$ExpectedCompactSha256,
    [switch]$RestoreNetworkOnly
)

if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or $env:RUNNER_OS -cne 'Windows') {
    throw 'This installer QA harness can run only on a GitHub-hosted Windows Actions runner.'
}

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$taskName = 'QlisaOfflineQaRestore-' + ($env:GITHUB_RUN_ID -replace '[^0-9]', '') + '-' + ($env:GITHUB_RUN_ATTEMPT -replace '[^0-9]', '')
$runFolder = 'qlisa-offline-qa-' + ($env:GITHUB_RUN_ID -replace '[^0-9]', '') + '-' + ($env:GITHUB_RUN_ATTEMPT -replace '[^0-9]', '')
$qaRoot = Join-Path $env:RUNNER_TEMP $runFolder
$statePath = Join-Path $qaRoot 'network-state.json'
$restoreScriptPath = Join-Path $qaRoot 'restore-network.ps1'

function Restore-NetworkFromState([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return }
    $state = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    $errors = [System.Collections.Generic.List[string]]::new()
    $mustBeUp = [System.Collections.Generic.List[string]]::new()
    foreach ($adapter in @($state.adapters)) {
        try {
            $current = Get-NetAdapter -InterfaceIndex ([int]$adapter.interfaceIndex) -IncludeHidden -ErrorAction SilentlyContinue
            if ($null -eq $current) { continue }
            if ($current.Status -eq 'Disabled') {
                Enable-NetAdapter -InputObject $current -Confirm:$false -ErrorAction Stop | Out-Null
            }
            if ($adapter.wasUp) { $mustBeUp.Add([string]$adapter.interfaceIndex) }
        } catch { $errors.Add("$($adapter.name): $($_.Exception.Message)") }
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($mustBeUp.Count -gt 0 -and [DateTime]::UtcNow -lt $deadline) {
        $pending = @($mustBeUp | Where-Object {
            $current = Get-NetAdapter -InterfaceIndex ([int]$_) -IncludeHidden -ErrorAction SilentlyContinue
            $null -ne $current -and $current.Status -ne 'Up'
        })
        if ($pending.Count -eq 0) { break }
        Start-Sleep -Milliseconds 500
    }
    foreach ($interfaceIndex in $mustBeUp) {
        $current = Get-NetAdapter -InterfaceIndex ([int]$interfaceIndex) -IncludeHidden -ErrorAction SilentlyContinue
        if ($null -ne $current -and $current.Status -ne 'Up') { $errors.Add("Interface ${interfaceIndex}: network adapter did not return to Up state") }
    }
    if ($errors.Count -gt 0) { throw ('Network restoration failed: ' + ($errors -join '; ')) }
    Set-Content -LiteralPath ($Path + '.restored') -Value ([DateTime]::UtcNow.ToString('o')) -Encoding ascii
}

function Remove-RestoreWatchdog {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
}

if ($RestoreNetworkOnly) {
    Restore-NetworkFromState $statePath
    Remove-RestoreWatchdog
    Write-Host 'Runner networking restoration check completed.'
    exit 0
}

if ($ReleaseTag -notmatch '^v?(\d+\.\d+\.\d+)$') { throw 'ReleaseTag must be a plain three-part version, optionally prefixed with v.' }
$version = $Matches[1]
if ($ExpectedFullSha256 -notmatch '^[A-Fa-f0-9]{64}$' -or $ExpectedCompactSha256 -notmatch '^[A-Fa-f0-9]{64}$') {
    throw 'Both expected installer SHA-256 inputs must contain exactly 64 hexadecimal characters.'
}
if ([string]::IsNullOrWhiteSpace($env:GH_TOKEN)) { throw 'GH_TOKEN is required to read draft release assets.' }
if ($env:GITHUB_REPOSITORY -cne 'Ruslan-mad/Qlisa') { throw 'This workflow must run only for the Qlisa repository.' }

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$fullName = "Qlisa_${version}_x64-full-setup.exe"
$compactName = "Qlisa_${version}_x64-update-setup.exe"
$downloadsDir = Join-Path $qaRoot 'downloads'
$reportDir = Join-Path $qaRoot 'report'
$installRoot = Join-Path $qaRoot 'install\Qlisa'
$transcriptPath = Join-Path $reportDir 'offline-qa.log'
$reportPath = Join-Path $reportDir 'offline-qa.json'
$helperPath = Join-Path $PSScriptRoot 'offline-installer-qa.mjs'
$nodeConfigPath = Join-Path $downloadsDir 'release-tauri-config.json'
$fullManifestPath = Join-Path $downloadsDir 'release-full-installer-manifest.json'
$script:report = [ordered]@{
    schemaVersion = 1
    status = 'running'
    releaseTag = $ReleaseTag
    version = $version
    runnerImage = $env:ImageOS
    runnerImageVersion = $env:ImageVersion
    workflowCommit = $env:GITHUB_SHA
    workflowRef = $env:GITHUB_REF
    startedAtUtc = [DateTime]::UtcNow.ToString('o')
    installers = [ordered]@{}
    runtime = [ordered]@{ screenshots = @() }
    phases = @()
    errors = @()
}
$script:transcriptStarted = $false
$script:exitCode = 0
$script:webViewPolicyPath = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
$script:webViewPolicyNames = @('qlisa.exe', 'com.qlisa.app')
$script:webViewPolicyBackup = $null
$script:webViewPolicyChanged = $false

function Add-Phase([string]$Name, [string]$Result, [double]$Seconds, [string]$Details = '') {
    [void]($script:report.phases += [ordered]@{
        name = $Name
        result = $Result
        seconds = [Math]::Round($Seconds, 2)
        details = $Details
        recordedAtUtc = [DateTime]::UtcNow.ToString('o')
    })
}

function Invoke-ProcessWithTimeout {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string]$ArgumentString,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [Parameter(Mandatory = $true)][string]$Label,
        [string]$WorkingDirectory = $env:RUNNER_TEMP,
        [int[]]$SuccessExitCodes = @(0, 3010)
    )
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $FilePath -ArgumentList $ArgumentString -WorkingDirectory $WorkingDirectory -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "$Label timed out after $TimeoutSeconds seconds."
    }
    $process.Refresh()
    $result = [ordered]@{ exitCode = $process.ExitCode; seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 2) }
    if ($process.ExitCode -notin $SuccessExitCodes) { throw "$Label failed with exit code $($process.ExitCode)." }
    $result.rebootRequired = ($process.ExitCode -eq 3010)
    return $result
}

function Invoke-Gh([string[]]$Arguments) {
    $output = & gh @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        $diagnostic = ($output | ForEach-Object { [string]$_ }) -join "`n"
        if (-not [string]::IsNullOrEmpty($env:GH_TOKEN)) { $diagnostic = $diagnostic.Replace($env:GH_TOKEN, '[redacted]') }
        if ($diagnostic.Length -gt 1000) { $diagnostic = $diagnostic.Substring(0, 1000) + '…' }
        throw "GitHub CLI request failed (exit $exitCode): $diagnostic"
    }
    return ($output -join "`n")
}

function Save-ReleaseSourceFile([string]$RepositoryPath, [string]$Destination) {
    $response = Invoke-Gh @('api', "repos/$env:GITHUB_REPOSITORY/contents/$RepositoryPath`?ref=$ReleaseTag") | ConvertFrom-Json
    if ($response.type -ne 'file' -or -not $response.content) { throw "Release tag source file is missing: $RepositoryPath." }
    $encoded = $response.content -replace '\s', ''
    $bytes = [Convert]::FromBase64String($encoded)
    [IO.File]::WriteAllBytes($Destination, $bytes)
}

function Assert-InstallerSignature([string]$Artifact, [string]$Signature) {
    $output = & node $helperPath verify-signature --artifact $Artifact --signature $Signature --config $nodeConfigPath --version $version 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) { throw "Tauri signature verification failed for $(Split-Path -Leaf $Artifact)." }
    return ($output -join "`n" | ConvertFrom-Json)
}

function Get-UninstallEntries([string]$Pattern) {
    $uninstallRoots = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    foreach ($root in $uninstallRoots) {
        Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
            Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -and $_.DisplayName -match $Pattern }
    }
}

function Get-WebView2Evidence {
    $paths = @()
    foreach ($base in @(
        (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\EdgeWebView\Application'),
        (Join-Path $env:ProgramFiles 'Microsoft\EdgeWebView\Application'),
        (Join-Path $env:LOCALAPPDATA 'Microsoft\EdgeWebView\Application')
    )) {
        if (Test-Path -LiteralPath $base -PathType Container) {
            foreach ($directory in Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue) {
                $runtime = Join-Path $directory.FullName 'msedgewebview2.exe'
                if (Test-Path -LiteralPath $runtime -PathType Leaf) {
                    $installer = Join-Path $directory.FullName 'Installer\setup.exe'
                    $paths += [pscustomobject]@{ runtime = $runtime; installer = $installer; version = $directory.Name }
                }
            }
        }
    }
    $entries = @(Get-UninstallEntries '(?i)Microsoft Edge WebView2 Runtime')
    return [pscustomobject]@{ paths = $paths; entries = $entries }
}

function Remove-WebView2FromRunner {
    $evidence = Get-WebView2Evidence
    if ($evidence.paths.Count -eq 0 -and $evidence.entries.Count -eq 0) {
        $script:report.runtime.webView2Removed = $true
        return
    }
    $installer = $evidence.paths | Where-Object { Test-Path -LiteralPath $_.installer -PathType Leaf } | Select-Object -First 1
    if ($null -eq $installer) {
        throw 'WebView2 is present but has no removable runtime installer. Cold offline QA cannot proceed.'
    }
    $scope = if ($installer.runtime.StartsWith($env:LOCALAPPDATA, [StringComparison]::OrdinalIgnoreCase)) { '--user-level' } else { '--system-level' }
    Invoke-ProcessWithTimeout -FilePath $installer.installer -ArgumentString "--uninstall --msedgewebview $scope --force-uninstall" -TimeoutSeconds 90 -Label 'WebView2 removal' -SuccessExitCodes @(0, 19, 3010) | Out-Null
    $after = Get-WebView2Evidence
    if ($after.paths.Count -gt 0 -or $after.entries.Count -gt 0) {
        throw 'WebView2 remains installed after removal. Cold offline QA cannot proceed.'
    }
    $script:report.runtime.webView2Removed = $true
}

function Get-NdiEvidence {
    $ndiPaths = @(
        (Join-Path ${env:ProgramFiles(x86)} 'NDI\NDI 6 Runtime\v6\Processing.NDI.Lib.x64.dll'),
        (Join-Path $env:ProgramFiles 'NDI\NDI 6 Runtime\v6\Processing.NDI.Lib.x64.dll'),
        (Join-Path $env:ProgramFiles 'NewTek\NDI\Processing.NDI.Lib.x64.dll')
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
    $entries = @(Get-UninstallEntries '(?i)((NDI|NewTek).*Runtime|Runtime.*(NDI|NewTek))')
    return [pscustomobject]@{ paths = @($ndiPaths); entries = $entries }
}

function Remove-NdiFromRunner {
    $evidence = Get-NdiEvidence
    if ($evidence.paths.Count -eq 0 -and $evidence.entries.Count -eq 0) {
        $script:report.runtime.ndiRemoved = $true
        return
    }
    $entry = $evidence.entries | Select-Object -First 1
    if ($null -eq $entry) { throw 'NDI DLL is present but no registered uninstaller was found; refusing file deletion.' }
    $quiet = [Environment]::ExpandEnvironmentVariables([string]$entry.QuietUninstallString)
    $uninstall = [Environment]::ExpandEnvironmentVariables([string]$entry.UninstallString)
    if ($quiet) { $commandLine = $quiet } else { $commandLine = $uninstall }
    if (-not $commandLine) { throw 'NDI is installed but has no registered uninstaller.' }
    if ($commandLine -match '(?i)msiexec(?:\.exe)?\s+(.+)') {
        $exe = Join-Path $env:WINDIR 'System32\msiexec.exe'
        $args = $Matches[1] -replace '(?i)(^|\s)/I(?=\s|\{)', '$1/X'
        if ($args -notmatch '(?i)/q') { $args += ' /qn' }
        if ($args -notmatch '(?i)/norestart') { $args += ' /norestart' }
    } elseif ($quiet -and $commandLine -match '^\s*"([^"]+)"\s*(.*)$') {
        $exe = $Matches[1]
        $args = $Matches[2]
    } elseif ($quiet -and $commandLine -match '^\s*(\S+)\s*(.*)$') {
        $exe = $Matches[1]
        $args = $Matches[2]
    } else {
        throw 'NDI has no quiet uninstall command; refusing an interactive uninstall.'
    }
    Invoke-ProcessWithTimeout -FilePath $exe -ArgumentString $args -TimeoutSeconds 120 -Label 'NDI Runtime removal' | Out-Null
    $after = Get-NdiEvidence
    if ($after.paths.Count -gt 0 -or $after.entries.Count -gt 0) {
        throw 'NDI remains installed after removal. Cold offline QA cannot proceed.'
    }
    $script:report.runtime.ndiRemoved = $true
}

function Write-RestoreScript {
    $body = @'
param([Parameter(Mandatory=$true)][string]$StatePath)
$ErrorActionPreference = 'Continue'
if (-not (Test-Path -LiteralPath $StatePath -PathType Leaf)) { exit 0 }
$state = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
$errors = @()
foreach ($adapter in @($state.adapters)) {
    try {
        $current = Get-NetAdapter -InterfaceIndex ([int]$adapter.interfaceIndex) -IncludeHidden -ErrorAction SilentlyContinue
        if ($null -eq $current) { continue }
        if ($current.Status -eq 'Disabled') {
            Enable-NetAdapter -InputObject $current -Confirm:$false -ErrorAction Stop | Out-Null
        }
    } catch { $errors += "$($adapter.name): $($_.Exception.Message)" }
}
if ($errors.Count -gt 0) { Set-Content -LiteralPath ($StatePath + '.restore-error') -Value ($errors -join '; ') -Encoding ascii; exit 1 }
$deadline = [DateTime]::UtcNow.AddSeconds(30)
while ([DateTime]::UtcNow -lt $deadline) {
    $pending = @($state.adapters | Where-Object {
        if (-not $_.wasUp) { return $false }
        $current = Get-NetAdapter -InterfaceIndex ([int]$_.interfaceIndex) -IncludeHidden -ErrorAction SilentlyContinue
        $null -ne $current -and $current.Status -ne 'Up'
    })
    if ($pending.Count -eq 0) { break }
    Start-Sleep -Milliseconds 500
}
$notUp = @($state.adapters | Where-Object {
    if (-not $_.wasUp) { return $false }
    $current = Get-NetAdapter -InterfaceIndex ([int]$_.interfaceIndex) -IncludeHidden -ErrorAction SilentlyContinue
    $null -ne $current -and $current.Status -ne 'Up'
})
if ($notUp.Count -gt 0) { Set-Content -LiteralPath ($StatePath + '.restore-error') -Value 'One or more network adapters did not return to Up state.' -Encoding ascii; exit 1 }
Set-Content -LiteralPath ($StatePath + '.restored') -Value ([DateTime]::UtcNow.ToString('o')) -Encoding ascii
'@
    Set-Content -LiteralPath $restoreScriptPath -Value $body -Encoding utf8
}

function Invoke-OfflineStage {
    param([string]$Name, [scriptblock]$Action, [int]$MaxSeconds = 210)
    $adapters = @(Get-NetAdapter -IncludeHidden | Where-Object { $_.Status -eq 'Up' -and $_.Name -notmatch '(?i)loopback' })
    if ($adapters.Count -eq 0) { throw 'No active non-loopback network adapter was found; cannot prove the offline stage.' }
    $state = [ordered]@{ adapters = @($adapters | ForEach-Object { [ordered]@{ name = $_.Name; interfaceIndex = [int]$_.ifIndex; wasEnabled = $true; wasUp = $true } }); createdAtUtc = [DateTime]::UtcNow.ToString('o') }
    Remove-Item -LiteralPath ($statePath + '.restored') -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath ($statePath + '.restore-error') -Force -ErrorAction SilentlyContinue
    $state | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $statePath -Encoding utf8
    Write-RestoreScript
    $trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5)
    $actionTask = New-ScheduledTaskAction -Execute (Join-Path $PSHOME 'pwsh.exe') -Argument "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$restoreScriptPath`" -StatePath `"$statePath`""
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    Register-ScheduledTask -TaskName $taskName -Action $actionTask -Trigger $trigger -Principal $principal -Force | Out-Null
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        foreach ($adapter in $adapters) {
            $current = Get-NetAdapter -InterfaceIndex ([int]$adapter.ifIndex) -IncludeHidden -ErrorAction SilentlyContinue
            if ($null -ne $current -and $current.Status -eq 'Up') {
                Disable-NetAdapter -InputObject $current -Confirm:$false -ErrorAction Stop | Out-Null
            }
        }
        $activeAdapters = @(Get-NetAdapter -IncludeHidden | Where-Object { $_.Status -eq 'Up' -and $_.Name -notmatch '(?i)loopback' })
        if ($activeAdapters.Count -gt 0) { throw 'One or more non-loopback network adapters remain active.' }
        $defaultRoutes = @(Get-NetRoute -ErrorAction SilentlyContinue | Where-Object {
            $_.DestinationPrefix -eq '0.0.0.0/0' -or $_.DestinationPrefix -eq '::/0'
        })
        if ($defaultRoutes.Count -gt 0) { throw 'One or more default routes remain during the offline stage.' }
        $script:report.lastOfflineStage = $Name
        Write-Host "OFFLINE: $Name; disabled $($adapters.Count) network adapter(s)."
        & $Action
        if ($timer.Elapsed.TotalSeconds -ge $MaxSeconds) { throw "$Name exceeded its $MaxSeconds second offline window." }
        Add-Phase $Name 'passed' $timer.Elapsed.TotalSeconds 'All active non-loopback adapters disabled; no default route remained.'
    } catch {
        Add-Phase $Name 'failed' $timer.Elapsed.TotalSeconds $_.Exception.Message
        throw
    } finally {
        Restore-NetworkFromState $statePath
        Remove-RestoreWatchdog
        if (-not (Test-Path -LiteralPath ($statePath + '.restored'))) { throw 'Network restore marker is missing after offline stage.' }
    }
}

function Get-AppProcess([string]$Executable) {
    $full = [IO.Path]::GetFullPath($Executable)
    Get-CimInstance Win32_Process -Filter "Name='qlisa.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath) -ieq $full }
}

function Stop-TestApp([string]$Executable) {
    foreach ($process in @(Get-AppProcess $Executable)) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline -and @(Get-AppProcess $Executable).Count -gt 0) { Start-Sleep -Milliseconds 250 }
    if (@(Get-AppProcess $Executable).Count -gt 0) { throw 'Could not stop the isolated Qlisa process.' }
}

function Set-WebView2DebugPolicy {
    if ($null -eq $script:webViewPolicyBackup) {
        $keyExisted = Test-Path -LiteralPath $script:webViewPolicyPath -PathType Container
        $originalValues = @()
        if ($keyExisted) {
            $key = Get-Item -LiteralPath $script:webViewPolicyPath
            $existingNames = @($key.GetValueNames())
            foreach ($name in $script:webViewPolicyNames) {
                if ($name -in $existingNames) {
                    $originalValues += [ordered]@{
                        name = $name
                        existed = $true
                        value = $key.GetValue($name)
                        kind = $key.GetValueKind($name).ToString()
                    }
                } else {
                    $originalValues += [ordered]@{ name = $name; existed = $false; value = $null; kind = $null }
                }
            }
        } else {
            foreach ($name in $script:webViewPolicyNames) {
                $originalValues += [ordered]@{ name = $name; existed = $false; value = $null; kind = $null }
            }
        }
        $script:webViewPolicyBackup = [ordered]@{ keyExisted = $keyExisted; values = $originalValues }
    }
    $script:webViewPolicyChanged = $true
    New-Item -Path $script:webViewPolicyPath -Force -ErrorAction Stop | Out-Null
    $arguments = '--remote-debugging-address=127.0.0.1 --remote-debugging-port=9222'
    foreach ($name in $script:webViewPolicyNames) {
        New-ItemProperty -LiteralPath $script:webViewPolicyPath -Name $name -Value $arguments -PropertyType String -Force -ErrorAction Stop | Out-Null
    }
    $script:report.runtime.remoteDebuggingPolicyApplied = $true
}

function Restore-WebView2DebugPolicy {
    if (-not $script:webViewPolicyChanged -or $null -eq $script:webViewPolicyBackup) { return }
    foreach ($value in $script:webViewPolicyBackup.values) {
        if ($value.existed) {
            New-ItemProperty -LiteralPath $script:webViewPolicyPath -Name $value.name -Value $value.value -PropertyType $value.kind -Force -ErrorAction Stop | Out-Null
        } else {
            Remove-ItemProperty -LiteralPath $script:webViewPolicyPath -Name $value.name -ErrorAction SilentlyContinue
        }
    }
    if (-not $script:webViewPolicyBackup.keyExisted -and (Test-Path -LiteralPath $script:webViewPolicyPath -PathType Container)) {
        $key = Get-Item -LiteralPath $script:webViewPolicyPath
        if (@($key.GetValueNames()).Count -eq 0 -and @($key.GetSubKeyNames()).Count -eq 0) {
            Remove-Item -LiteralPath $script:webViewPolicyPath -Force -ErrorAction Stop
        }
    }
    $script:webViewPolicyChanged = $false
    $script:report.runtime.remoteDebuggingPolicyRestored = $true
}

function Start-AppAndProbe([string]$Executable, [string]$CheckName, [string]$ScreenshotPath) {
    Stop-TestApp $Executable
    $portDeadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $portUse = Get-NetTCPConnection -LocalPort 9222 -State Listen -ErrorAction SilentlyContinue
        if ($portUse) { Start-Sleep -Milliseconds 250 }
    } while ($portUse -and [DateTime]::UtcNow -lt $portDeadline)
    if ($portUse) { throw 'CDP port 9222 is already in use on the ephemeral runner.' }
    $logStem = [regex]::Replace($CheckName, '[^A-Za-z0-9_-]', '-')
    $stdoutPath = Join-Path $reportDir "$logStem.stdout.log"
    $stderrPath = Join-Path $reportDir "$logStem.stderr.log"
    $oldArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    try {
        Set-WebView2DebugPolicy
        Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
        $process = Start-Process -FilePath $Executable -PassThru -WorkingDirectory (Split-Path -Parent $Executable) -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    } finally {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldArgs
    }
    $out = & node $helperPath probe --port 9222 --timeout-ms 90000 --screenshot $ScreenshotPath 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        $process.Refresh()
        $startedProcessHasExited = $process.HasExited
        $startedProcessExitCode = if ($startedProcessHasExited) { $process.ExitCode } else { $null }
        $startedProcessSessionId = try { $process.SessionId } catch { $null }
        $appProcesses = @(Get-AppProcess $Executable | Select-Object ProcessId, ParentProcessId, SessionId, ExecutablePath, CommandLine)
        $appProcessIds = @($appProcesses | ForEach-Object { [uint32]$_.ProcessId })
        $webViewProcesses = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.ParentProcessId -in $appProcessIds } |
            Select-Object ProcessId, ParentProcessId, SessionId, ExecutablePath, CommandLine)
        $loadedModules = @()
        foreach ($appProcessId in $appProcessIds) {
            try {
                $loadedModules += @(Get-Process -Id $appProcessId -Module -ErrorAction Stop |
                    Where-Object { $_.ModuleName -match '^(?i:libmpv|vulkan-1)\.dll$' } |
                    Select-Object @{ Name = 'ProcessId'; Expression = { $appProcessId } }, ModuleName, FileName)
            } catch { }
        }
        $listeners = @(Get-NetTCPConnection -LocalPort 9222 -State Listen -ErrorAction SilentlyContinue |
            Select-Object LocalAddress, LocalPort, OwningProcess)
        $applicationErrors = @()
        try {
            $eventStart = ([DateTime]::Parse($script:report.startedAtUtc)).ToLocalTime()
            $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'Application Error'; StartTime = $eventStart } -MaxEvents 50 -ErrorAction SilentlyContinue |
                Where-Object { $_.Message -match '(?i)(qlisa\.exe|msedgewebview2\.exe)' } | Select-Object -First 10)
            foreach ($event in $events) {
                $message = [string]$event.Message
                if ($message.Length -gt 2000) { $message = $message.Substring(0, 2000) + '…' }
                $applicationErrors += [ordered]@{ timeCreated = $event.TimeCreated.ToString('o'); eventId = $event.Id; provider = $event.ProviderName; message = $message }
            }
        } catch { }
        $appLogSource = Join-Path $env:APPDATA 'Inkue\logs\inkue.log'
        $appLogName = 'inkue-application.log'
        $appLogCopied = $false
        if (Test-Path -LiteralPath $appLogSource -PathType Leaf) {
            try { Copy-Item -LiteralPath $appLogSource -Destination (Join-Path $reportDir $appLogName) -Force; $appLogCopied = $true } catch { }
        }
        $script:report.runtime['appProbeDiagnostics'] = [ordered]@{
            check = $CheckName
            startedProcessHasExited = $startedProcessHasExited
            startedProcessExitCode = $startedProcessExitCode
            startedProcessSessionId = $startedProcessSessionId
            appProcesses = $appProcesses
            childWebViewProcesses = $webViewProcesses
            loadedMediaModules = $loadedModules
            cdpListeners = $listeners
            applicationErrorEvents = $applicationErrors
            appLogCopied = $appLogCopied
            appLog = if ($appLogCopied) { $appLogName } else { $null }
            stdoutLog = (Split-Path -Leaf $stdoutPath)
            stderrLog = (Split-Path -Leaf $stderrPath)
        }
        $diagnosticOutput = (($out | ForEach-Object { [string]$_ }) -join "`n")
        if ($diagnosticOutput.Length -gt 2000) { $diagnosticOutput = $diagnosticOutput.Substring(0, 2000) + '…' }
        Stop-TestApp $Executable
        throw "$CheckName failed: $diagnosticOutput"
    }
    $probeResult = ($out -join "`n") | ConvertFrom-Json
    if (-not $probeResult.ok -or -not $probeResult.backendReady -or -not $probeResult.media.ready -or $probeResult.network.ndi.availability -ne 'ready') {
        throw "$CheckName returned an unexpected readiness response."
    }
    Add-Phase $CheckName 'passed' 0 'Backend ready; FFmpeg, ffprobe, libmpv and NDI reported ready through CDP.'
    if ($probeResult.screenshotWritten) { $script:report.runtime.screenshots += (Split-Path -Leaf $ScreenshotPath) }
    Stop-TestApp $Executable
    return $probeResult
}

function Assert-MediaRuntimeCache([string]$Label, $Manifest) {
    $cache = Join-Path $env:LOCALAPPDATA 'Qlisa\runtime'
    $records = @()
    foreach ($record in $Manifest.runtimeFiles) {
        $path = Join-Path $cache $record.fileName
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "$Label missing runtime file $($record.fileName)." }
        $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($hash -ne $record.sha256.ToLowerInvariant()) { throw "$Label runtime hash mismatch for $($record.fileName)." }
        $records += [ordered]@{ fileName = $record.fileName; sha256 = $hash }
    }
    $script:report.runtime[$Label] = $records
}

function Assert-WebView2Installed {
    $evidence = Get-WebView2Evidence
    if ($evidence.paths.Count -eq 0) { throw 'WebView2 runtime was not installed by the full offline installer.' }
    $runtimePath = $evidence.paths[0].runtime
    $signature = Get-AuthenticodeSignature -LiteralPath $runtimePath
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft') {
        throw 'Installed WebView2 runtime does not have a valid Microsoft Authenticode signature.'
    }
    $observed = [ordered]@{
        version = $evidence.paths[0].version
        runtimePath = $runtimePath
        signer = $signature.SignerCertificate.Subject
        sha256 = (Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if ($script:report.runtime.Contains('webView2')) {
        $previous = $script:report.runtime['webView2']
        if ($previous.runtimePath -ne $observed.runtimePath -or $previous.sha256 -ne $observed.sha256) {
            throw 'Compact update changed or removed the installed WebView2 runtime.'
        }
    }
    $script:report.runtime.webView2 = $observed
}

function Assert-NdiInstalled($Manifest) {
    $evidence = Get-NdiEvidence
    if ($evidence.paths.Count -eq 0) { throw 'NDI runtime DLL is missing after the full offline installer.' }
    $dll = Get-Item -LiteralPath $evidence.paths[0]
    $actualVersion = $dll.VersionInfo.FileVersion
    if ($actualVersion -ne $Manifest.ndiRuntime.version) { throw "NDI version mismatch: expected $($Manifest.ndiRuntime.version), got $actualVersion." }
    $observed = [ordered]@{
        version = $actualVersion
        runtimePath = $dll.FullName
        sha256 = (Get-FileHash -LiteralPath $dll.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if ($script:report.runtime.Contains('ndi')) {
        $previous = $script:report.runtime['ndi']
        if ($previous.runtimePath -ne $observed.runtimePath -or $previous.sha256 -ne $observed.sha256) {
            throw 'Compact update changed or removed the global NDI Runtime.'
        }
    }
    $script:report.runtime.ndi = $observed
}

function Write-FinalReport {
    $script:report.finishedAtUtc = [DateTime]::UtcNow.ToString('o')
    if ($script:report.errors.Count -gt 0) { $script:report.status = 'failed' } else { $script:report.status = 'passed' }
    $script:report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $reportPath -Encoding utf8
}

try {
    New-Item -ItemType Directory -Path $qaRoot -ErrorAction Stop | Out-Null
    New-Item -ItemType Directory -Path $downloadsDir -ErrorAction Stop | Out-Null
    New-Item -ItemType Directory -Path $reportDir -ErrorAction Stop | Out-Null
    New-Item -ItemType Directory -Path (Split-Path -Parent $installRoot) -ErrorAction Stop | Out-Null
    if ($installRoot -match '\s') { throw 'The isolated install path must not contain spaces for NSIS /D parsing.' }
    Start-Transcript -Path $transcriptPath -Force | Out-Null
    $script:transcriptStarted = $true

    $tagVersion = $version
    $fullName = "Qlisa_${tagVersion}_x64-full-setup.exe"
    $compactName = "Qlisa_${tagVersion}_x64-update-setup.exe"
    $releaseInfo = Invoke-Gh @('release', 'view', $ReleaseTag, '--repo', $env:GITHUB_REPOSITORY, '--json', 'tagName,isDraft,isPrerelease,url') | ConvertFrom-Json
    if ($releaseInfo.tagName -ne $ReleaseTag -or -not $releaseInfo.isDraft -or $releaseInfo.isPrerelease) {
        throw 'The requested GitHub release must exist as a non-prerelease draft with the exact tag.'
    }
    $script:report.releaseUrl = $releaseInfo.url
    Save-ReleaseSourceFile 'scripts/full-installer-manifest.json' $fullManifestPath
    Save-ReleaseSourceFile 'src-tauri/tauri.conf.json' $nodeConfigPath
    $downloadArgs = @('release', 'download', $ReleaseTag, '--repo', $env:GITHUB_REPOSITORY, '--dir', $downloadsDir)
    foreach ($asset in @($fullName, "$fullName.sig", $compactName, "$compactName.sig", 'latest.json')) { $downloadArgs += @('--pattern', $asset) }
    Invoke-Gh $downloadArgs | Out-Null

    $fullPath = Join-Path $downloadsDir $fullName
    $fullSigPath = "$fullPath.sig"
    $compactPath = Join-Path $downloadsDir $compactName
    $compactSigPath = "$compactPath.sig"
    $metadataPath = Join-Path $downloadsDir 'latest.json'
    foreach ($path in @($fullPath, $fullSigPath, $compactPath, $compactSigPath, $metadataPath)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Draft release asset is missing: $(Split-Path -Leaf $path)." }
    }

    $actualFullHash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $actualCompactHash = (Get-FileHash -LiteralPath $compactPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualFullHash -ne $ExpectedFullSha256.ToLowerInvariant()) { throw 'Full installer SHA-256 does not match the dispatch input.' }
    if ($actualCompactHash -ne $ExpectedCompactSha256.ToLowerInvariant()) { throw 'Compact installer SHA-256 does not match the dispatch input.' }
    $fullSignatureResult = Assert-InstallerSignature $fullPath $fullSigPath
    $compactSignatureResult = Assert-InstallerSignature $compactPath $compactSigPath
    $latest = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
    $latestPlatform = $latest.platforms.'windows-x86_64'
    if (($latest.version -replace '^v', '') -ne $version) { throw 'latest.json version does not match the requested release tag.' }
    if ($latestPlatform.url -notlike "*$ReleaseTag/$compactName" -or $latestPlatform.signature.Trim() -ne (Get-Content -LiteralPath $compactSigPath -Raw).Trim()) {
        throw 'latest.json does not point to the verified compact installer and signature.'
    }
    $script:report.installers.full = [ordered]@{ name = $fullName; sha256 = $actualFullHash; signatureVerified = $fullSignatureResult.verified; signatureAlgorithm = $fullSignatureResult.algorithm }
    $script:report.installers.compact = [ordered]@{ name = $compactName; sha256 = $actualCompactHash; signatureVerified = $compactSignatureResult.verified; signatureAlgorithm = $compactSignatureResult.algorithm }
    $script:report.installers.latestJsonChecked = $true
    Write-Host "Verified draft $ReleaseTag assets, SHA-256 pins and Tauri signatures."

    $fullManifest = Get-Content -LiteralPath $fullManifestPath -Raw | ConvertFrom-Json
    if ($fullManifest.schemaVersion -ne 1) { throw 'Unsupported full-installer manifest schema.' }
    $baselineCache = Join-Path $env:LOCALAPPDATA 'Qlisa\runtime'
    if (Test-Path -LiteralPath $baselineCache -PathType Container) {
        $baselineFiles = @(Get-ChildItem -LiteralPath $baselineCache -Force -Recurse -File -ErrorAction SilentlyContinue)
        if ($baselineFiles.Count -gt 0) { throw 'Runner already has a Qlisa runtime cache; cold offline QA requires an empty cache.' }
    }
    $script:report.runtime.baselineCacheAbsent = $true
    Remove-WebView2FromRunner
    Remove-NdiFromRunner
    $preInstallWebView = Get-WebView2Evidence
    $preInstallNdi = Get-NdiEvidence
    if ($preInstallWebView.paths.Count -gt 0 -or $preInstallWebView.entries.Count -gt 0 -or $preInstallNdi.paths.Count -gt 0 -or $preInstallNdi.entries.Count -gt 0) {
        throw 'The clean runner baseline still contains WebView2 or NDI; cold offline QA cannot proceed.'
    }

    Invoke-OfflineStage -Name 'full-install-cold-offline' -Action {
        $fullInstall = Invoke-ProcessWithTimeout -FilePath $fullPath -ArgumentString "/S /D=$installRoot" -TimeoutSeconds 180 -Label 'Full installer'
        $script:report.installers.fullInstall = $fullInstall
        $fullMarker = Join-Path $installRoot 'full-runtime\full-package.json'
        if (-not (Test-Path -LiteralPath $fullMarker -PathType Leaf)) { throw 'Full installer seed marker is missing after installation.' }
        $markerHash = (Get-FileHash -LiteralPath $fullMarker -Algorithm SHA256).Hash.ToLowerInvariant()
        $expectedMarkerHash = (Get-FileHash -LiteralPath $fullManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($markerHash -ne $expectedMarkerHash) { throw 'Installed full-package.json does not match the release-tag manifest.' }
        foreach ($record in $fullManifest.runtimeFiles) {
            $payload = Join-Path (Split-Path -Parent $fullMarker) $record.fileName
            if (-not (Test-Path -LiteralPath $payload -PathType Leaf)) { throw "Full installer payload is missing $($record.fileName)." }
            $payloadHash = (Get-FileHash -LiteralPath $payload -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($payloadHash -ne $record.sha256.ToLowerInvariant()) { throw "Full installer payload hash mismatch for $($record.fileName)." }
        }
        $ndiPayload = Join-Path $installRoot 'ndi\NDI-6-Runtime.exe'
        if (-not (Test-Path -LiteralPath $ndiPayload -PathType Leaf)) { throw 'Full installer did not preserve its staged NDI runtime installer resource.' }
        $ndiHash = (Get-FileHash -LiteralPath $ndiPayload -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($ndiHash -ne $fullManifest.ndiRuntime.sha256.ToLowerInvariant()) { throw 'Bundled NDI installer hash mismatch.' }
        $ndiSig = Get-AuthenticodeSignature -LiteralPath $ndiPayload
        if ($ndiSig.Status -ne 'Valid' -or $ndiSig.SignerCertificate.Subject -notmatch [regex]::Escape($fullManifest.ndiRuntime.signer)) { throw 'Bundled NDI installer Authenticode signature is invalid.' }
        if (-not (Test-Path -LiteralPath (Join-Path $installRoot 'qlisa.exe') -PathType Leaf)) { throw 'Full installer did not create qlisa.exe in the requested test directory.' }
        Assert-WebView2Installed
        Assert-NdiInstalled $fullManifest
        $ndiLog = Join-Path $env:TEMP 'Qlisa-NDI-Runtime-install.log'
        if (Test-Path -LiteralPath $ndiLog -PathType Leaf) { Copy-Item -LiteralPath $ndiLog -Destination (Join-Path $reportDir 'ndi-install.log') -Force }
    }

    $appPath = Join-Path $installRoot 'qlisa.exe'
    $runtimeCache = Join-Path $env:LOCALAPPDATA 'Qlisa\runtime'
    Invoke-OfflineStage -Name 'first-boot-seed-and-warm-restart' -Action {
        $first = Start-AppAndProbe $appPath 'full-first-boot' (Join-Path $reportDir 'full-first-boot.png')
        Assert-MediaRuntimeCache 'afterFullInstall' $fullManifest
        $script:report.runtime.firstBootStatus = $first
        $warm = Start-AppAndProbe $appPath 'full-warm-restart' (Join-Path $reportDir 'full-warm-restart.png')
        $script:report.runtime.warmRestartStatus = $warm
    }

    Stop-TestApp $appPath
    Invoke-OfflineStage -Name 'compact-over-full-install' -Action {
        $compactInstall = Invoke-ProcessWithTimeout -FilePath $compactPath -ArgumentString "/S /UPDATE /D=$installRoot" -TimeoutSeconds 180 -Label 'Compact update installer'
        $script:report.installers.compactInstall = $compactInstall
        $fullMarker = Join-Path $installRoot 'full-runtime\full-package.json'
        if (Test-Path -LiteralPath $fullMarker -PathType Leaf) { throw 'Compact update left the full-installer seed marker in the application directory.' }
        $fullPayload = Join-Path $installRoot 'full-runtime'
        if (Test-Path -LiteralPath $fullPayload -PathType Container) {
            $leftovers = @(Get-ChildItem -LiteralPath $fullPayload -Force -ErrorAction SilentlyContinue)
            if ($leftovers.Count -gt 0) { throw 'Compact update left full-installer payload files behind.' }
        }
        if (-not (Test-Path -LiteralPath $runtimeCache -PathType Container)) { throw 'Compact update removed the runtime cache.' }
        Assert-MediaRuntimeCache 'afterCompactInstall' $fullManifest
        Assert-WebView2Installed
        Assert-NdiInstalled $fullManifest
    }

    Invoke-OfflineStage -Name 'post-update-cold-boot' -Action {
        $post = Start-AppAndProbe $appPath 'compact-post-update-boot' (Join-Path $reportDir 'compact-post-update.png')
        $script:report.runtime.postUpdateStatus = $post
        Assert-MediaRuntimeCache 'afterCompactBoot' $fullManifest
        Assert-WebView2Installed
        Assert-NdiInstalled $fullManifest
    }
    $script:report.status = 'passed'
} catch {
    [void]($script:report.errors += $_.Exception.Message)
    Write-Host "QA failed: $($_.Exception.Message)" -ForegroundColor Red
    $script:exitCode = 1
} finally {
    $networkRestored = $false
    try { Restore-NetworkFromState $statePath; $networkRestored = $true } catch { [void]($script:report.errors += $_.Exception.Message); $script:report.status = 'failed' }
    if ($networkRestored) { Remove-RestoreWatchdog }
    try { Restore-WebView2DebugPolicy } catch { [void]($script:report.errors += ('WebView2 debug policy restoration failed: ' + $_.Exception.Message)); $script:report.status = 'failed' }
    if ($script:transcriptStarted) { Stop-Transcript | Out-Null }
    if (Test-Path -LiteralPath $reportDir -PathType Container) {
        try { Write-FinalReport } catch { Write-Error "Could not write final QA report: $($_.Exception.Message)" }
    }
}
if ($script:exitCode -ne 0 -or $script:report.status -ne 'passed') { exit 1 }
Write-Host "Offline QA passed. Report: $reportPath"
