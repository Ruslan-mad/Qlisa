<#
.SYNOPSIS
Compatibility entry point for the pinned Windows FFmpeg runtime staging script.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$prepareRuntime = Join-Path $PSScriptRoot 'prepare-runtime.ps1'
& $prepareRuntime
