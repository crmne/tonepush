# Fail if a Windows executable imports the C runtime instead of carrying it.
# A build can pass while importing VCRUNTIME140.dll and still exit 0xC0000135 on
# a clean machine, so this reads the imports of the executables themselves.
#
#   pwsh packaging/windows/check-runtime.ps1 path\to\tonepush.exe ...
param(
    [Parameter(Mandatory, ValueFromRemainingArguments)]
    [string[]] $Executables
)

$ErrorActionPreference = 'Stop'

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$dumpbin = & $vswhere -latest -products * -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' |
    Select-Object -First 1
if (-not $dumpbin) { throw 'dumpbin.exe not found' }

$runtime = '^(vcruntime|msvcp|msvcr|concrt|vccorlib|vcomp|ucrtbase|api-ms-win-crt-)'
$failed = $false

foreach ($executable in $Executables) {
    if (-not (Test-Path -PathType Leaf $executable)) { throw "$executable does not exist" }
    $output = & $dumpbin /nologo /dependents $executable
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on ${executable}: $output" }

    $imports = @($output | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' })
    # Every Windows program imports KERNEL32; without it this read nothing, and
    # an empty list must not pass.
    if ($imports -notcontains 'KERNEL32.dll') { throw "no imports read from ${executable}: $output" }

    "$executable imports: $($imports -join ', ')"
    $found = @($imports | Where-Object { $_ -match $runtime })
    if ($found) {
        Write-Host "::error::$executable imports the C runtime: $($found -join ', ')"
        $failed = $true
    }
}

if ($failed) { exit 1 }
