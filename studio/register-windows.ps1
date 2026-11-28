param(
    [Parameter(Mandatory)]
    [string]$Executable
)

$ErrorActionPreference = 'Stop'
$studio = Get-Item -LiteralPath $Executable
if ($studio.PSIsContainer -or $studio.Extension -ne '.exe') {
    throw 'Choose the PSF Studio executable.'
}

$protocol = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Classes\psf-studio')
try {
    $protocol.SetValue('', 'URL:PSF Studio')
    $protocol.SetValue('URL Protocol', '')
    $command = $protocol.CreateSubKey('shell\open\command')
    try {
        $command.SetValue('', ('"{0}" --open-link "%1"' -f $studio.FullName))
    }
    finally {
        $command.Dispose()
    }
}
finally {
    $protocol.Dispose()
}
