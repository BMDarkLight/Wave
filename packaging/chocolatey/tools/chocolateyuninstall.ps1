# Wave — Chocolatey uninstall script
# Copyright (C) 2025 BMDarkLight — AGPL-3.0-or-later

$ErrorActionPreference = 'Stop'

$packageName = 'wave'
$softwareName = 'Wave*'

[array] $key = Get-UninstallRegistryKey -SoftwareName $softwareName

if ($key.Count -eq 1) {
    # The NSIS uninstaller takes the wave command back off PATH on its way out.
    Uninstall-ChocolateyPackage -PackageName $packageName `
        -FileType 'exe' `
        -SilentArgs '/S' `
        -ValidExitCodes @(0) `
        -File $key[0].UninstallString.Trim('"')
}
elseif ($key.Count -eq 0) {
    Write-Warning "$packageName is not installed; nothing to uninstall."
}
else {
    Write-Warning "$($key.Count) matches found for '$softwareName'. Uninstall them by hand:"
    $key | ForEach-Object { Write-Warning "  $($_.DisplayName)" }
}
