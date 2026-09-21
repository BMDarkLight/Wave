# Wave — Chocolatey install script
# Copyright (C) 2025 BMDarkLight — AGPL-3.0-or-later
#
# Checksum is a placeholder. Fill it from the value the release workflow prints
# for the .exe asset before pushing the package.

$ErrorActionPreference = 'Stop'

$version = '0.5.0'
$packageArgs = @{
    packageName    = 'wave'
    fileType       = 'exe'
    url64bit       = "https://github.com/BMDarkLight/Wave/releases/download/v$version/Wave_${version}_x64-setup.exe"
    checksum64     = '0000000000000000000000000000000000000000000000000000000000000000'
    checksumType64 = 'sha256'

    # NSIS silent switch. The installer adds its bin directory to PATH itself,
    # so nothing extra is needed here.
    silentArgs     = '/S'
    validExitCodes = @(0)

    softwareName   = 'Wave*'
}

Install-ChocolateyPackage @packageArgs
