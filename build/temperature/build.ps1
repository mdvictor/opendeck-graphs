# Build the optional Windows tray provider against an extracted vendor release.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $LibreHardwareMonitorFolder,
    [Parameter(Mandatory)] [string] $OutputFolder
)
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path $LibreHardwareMonitorFolder).Path
$null = New-Item -ItemType Directory -Force -Path $OutputFolder
$output = (Resolve-Path $OutputFolder).Path
$compiler = Join-Path $env:windir 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $compiler /nologo /target:winexe /platform:x64 "/out:$output\OpenDeckCpuTemperature.exe" "/reference:$source\LibreHardwareMonitorLib.dll" /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.ServiceProcess.dll "$PSScriptRoot\CpuTemperature.cs"
if ($LASTEXITCODE) { throw "C# compiler exited with $LASTEXITCODE" }
Copy-Item -LiteralPath "$source\LibreHardwareMonitor.exe.config" -Destination "$output\OpenDeckCpuTemperature.exe.config"
