[CmdletBinding()]
param([string] $Cargo = 'cargo', [string] $CrtDirectory)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Push-Location $root
try {
    $triple = 'x86_64-pc-windows-msvc'
    if (!$CrtDirectory) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        $crt = Get-ChildItem "$vs/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT/vcruntime140.dll" |
            Sort-Object { $_.VersionInfo.FileVersionRaw } -Descending | Select-Object -First 1
        if (!$crt) { throw 'Supply -CrtDirectory pointing to the x64 VC release redistributables.' }
        $CrtDirectory = $crt.DirectoryName
    }
    & $Cargo build --locked --release --target $triple
    if ($LASTEXITCODE) { throw "Cargo build exited with $LASTEXITCODE" }
    $metadata = & $Cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE) { throw "Cargo metadata exited with $LASTEXITCODE" }
    $target = ($metadata | ConvertFrom-Json).target_directory
    $id = 'com.stevengriffin.interstitial.graphs.sdPlugin'
    $manifest = Get-Content -LiteralPath manifest.json -Raw | ConvertFrom-Json -AsHashtable
    $stage = Join-Path $root "target/package/$([Guid]::NewGuid())/$id"
    $null = New-Item -ItemType Directory -Path "$stage/img" -Force
    $manifest.Actions += @((Get-Content -LiteralPath 'launcher-action.json' -Raw | ConvertFrom-Json -AsHashtable))
    $manifest.OS = @($manifest.OS | Where-Object Platform -eq windows)
    $manifest.CodePaths = @{$triple = $manifest.CodePathWin}
    $manifest.Remove('CodePathLin')
    [IO.File]::WriteAllText("$stage/manifest.json", ($manifest | ConvertTo-Json -Depth 12))
    Copy-Item -LiteralPath "$target/$triple/release/oa-graphs.exe" -Destination $stage
    Copy-Item -LiteralPath (Join-Path $CrtDirectory 'vcruntime140.dll') -Destination $stage
    Copy-Item -LiteralPath 'img/icon.png' -Destination "$stage/img"
    Copy-Item -LiteralPath 'pi.html','launcher.html','inspector.css','strip.json','LICENSE','README.md','fonts/LICENSE-DejaVu.txt' -Destination $stage
    $out = Join-Path $root "artifacts"
    $null = New-Item -ItemType Directory -Path $out -Force
    $zip = "$out/Interstitial-Hardware-Graphs-$($manifest.Version)-win-x64.zip"
    $temp = "$stage.zip"
    [IO.Compression.ZipFile]::CreateFromDirectory($stage,$temp,[IO.Compression.CompressionLevel]::Optimal,$true)
    [IO.File]::Move($temp,$zip,$true)
    Get-Item -LiteralPath $zip
} finally {
    Pop-Location
}
