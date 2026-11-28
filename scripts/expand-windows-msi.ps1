param(
    [Parameter(Mandatory)]
    [string]$Package,
    [Parameter(Mandatory)]
    [string]$Destination
)

$ErrorActionPreference = 'Stop'
$packagePath = (Resolve-Path -LiteralPath $Package).Path
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$arguments = '/a "{0}" /qn /norestart TARGETDIR="{1}"' -f $packagePath, $destinationPath
$process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "MSI extraction failed with exit code $($process.ExitCode)."
}
