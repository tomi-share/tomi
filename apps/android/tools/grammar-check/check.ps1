<#
.SYNOPSIS
  Host test (Windows, MSVC Build Tools) of the PHP highlighter of the app: builds highlight_core.cpp with the real
  grammars and prints a sample file with its coloured runs. Same C++ as libtomi_highlight.so, so a query error shows up here.

.EXAMPLE
  .\check.ps1 -Language php -File .\samples\sample.php
  .\check.ps1 -Language php -File .\samples\sample.php -Tokens

  -Grammars: folder with tree-sitter/, and tree-sitter-php/ (default: the cache of the Gradle task
  fetchGrammarSources, apps/android/.grammar-sources). Build outputs go to ./build (ignored by git).
#>
param(
    [Parameter(Mandatory = $true)][ValidateSet('php')][string]$Language,
    [Parameter(Mandatory = $true)][string]$File,
    [string]$Grammars = (Join-Path $PSScriptRoot '..\..\.grammar-sources'),
    [switch]$Tokens,
    [switch]$Rebuild
)
$ErrorActionPreference = 'Stop'
$Grammars = (Resolve-Path $Grammars).Path
$cpp = (Resolve-Path (Join-Path $PSScriptRoot '..\..\app\src\main\cpp')).Path
$out = Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force $out, "$out\gen" | Out-Null

# MSVC environment.
$vcvars = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path $vcvars)) { throw "vcvars64.bat not found: install the Visual Studio Build Tools (C++)." }
$env:PATH += ";${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer"
$ErrorActionPreference = 'Continue'
cmd /c "`"$vcvars`" >nul 2>nul && set" 2>$null | ForEach-Object { if ($_ -match '^([^=]+)=(.*)$') { Set-Item "env:$($Matches[1])" $Matches[2] } }
$ErrorActionPreference = 'Stop'

# Queries -> header (same shape as the one the CMake file generates).
$php = [IO.File]::ReadAllText("$cpp\queries\php.scm")
$header = "static const char kPhpQuery[] = R`"TSQUERY($php)TSQUERY`";`n"
[IO.File]::WriteAllText("$out\gen\queries_gen.h", $header)

function Compile($src, $obj, $incs) {
    if (-not $Rebuild -and (Test-Path $obj) -and ((Get-Item $obj).LastWriteTime -gt (Get-Item $src).LastWriteTime)) { return }
    $args = @('/nologo', '/c', '/O1', '/w', '/MD', '/std:c11', "/Fo$obj") + ($incs | ForEach-Object { "/I$_" }) + @($src)
    Write-Host "cc $src"
    & cl @args | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "compilation failed: $src" }
}

$ts = "$Grammars\tree-sitter"
Compile "$ts\lib\src\lib.c" "$out\ts.obj" @("$ts\lib\include", "$ts\lib\src")
Compile "$Grammars\tree-sitter-php\php\src\parser.c" "$out\php_parser.obj" @("$Grammars\tree-sitter-php\php\src")
Compile "$Grammars\tree-sitter-php\php\src\scanner.c" "$out\php_scanner.obj" @("$Grammars\tree-sitter-php\php\src")

Write-Host "cl check.exe"
& cl /nologo /EHsc /O1 /std:c++17 /MD "/I$out\gen" "/I$ts\lib\include" "/Fo$out\\" "/Fe$out\check.exe" `
    "$PSScriptRoot\main.cpp" "$cpp\highlight_core.cpp" `
    "$out\ts.obj" "$out\php_parser.obj" "$out\php_scanner.obj" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "link failed" }

$extra = @(); if ($Tokens) { $extra += '--tokens' }
& "$out\check.exe" $Language (Resolve-Path $File).Path @extra
