# Use the standard Rust/MSVC installation or an optional local portable MSVC.
# This helper does not install software or download packages itself.
$ErrorActionPreference = 'Stop'
$projectDirectory = Split-Path -Parent $PSScriptRoot
$portableDirectory = Join-Path $projectDirectory '.tools/msvc'
$cargoBinary = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'
if (-not (Test-Path -LiteralPath $cargoBinary)) { $cargoBinary = 'cargo' }
if (Test-Path -LiteralPath (Join-Path $portableDirectory 'setup_x64.bat')) {
    $setupPath = Join-Path $portableDirectory 'setup_x64.bat'
    $compilerEnvironment = & $env:ComSpec /d /s /c "`"`"$setupPath`" >nul && set`""
    if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the local MSVC environment' }
    foreach ($line in $compilerEnvironment) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
}
& $cargoBinary @args
exit $LASTEXITCODE
