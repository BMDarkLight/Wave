<#
    Wave — Copyright (C) 2025 BMDarkLight
    Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
    See the LICENSE file in the project root for the full license text.
    https://github.com/BMDarkLight/Wave

    Adds or removes a directory on the persistent PATH.

    The installer calls this instead of editing PATH from NSIS directly. NSIS
    strings are capped at 1024 characters (NSIS_MAX_STRLEN), so reading a PATH
    longer than that truncates it, and writing the truncated value back silently
    destroys the rest of the user's PATH. Going through the registry APIs here
    has no such limit.

    The raw value is read with DoNotExpandEnvironmentNames and written back with
    its original value kind, so REG_EXPAND_SZ entries such as %USERPROFILE%\bin
    survive a round trip instead of being frozen to their expanded form.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('add', 'remove')]
    [string] $Action,

    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()]
    [string] $Directory,

    [ValidateSet('User', 'Machine')]
    [string] $Scope = 'User'
)

$ErrorActionPreference = 'Stop'

function Open-EnvironmentKey {
    param([string] $Scope)

    if ($Scope -eq 'Machine') {
        $hive = [Microsoft.Win32.Registry]::LocalMachine
        $path = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
    }
    else {
        $hive = [Microsoft.Win32.Registry]::CurrentUser
        $path = 'Environment'
    }

    $key = $hive.OpenSubKey($path, $true)
    if ($null -eq $key) { $key = $hive.CreateSubKey($path) }
    return $key
}

# Compare entries the way the shell resolves them: case-insensitively, ignoring
# a trailing separator, so C:\Foo and c:\foo\ are recognised as the same entry.
function Test-SameDirectory {
    param([string] $A, [string] $B)
    $a = $A.Trim().TrimEnd('\', '/')
    $b = $B.Trim().TrimEnd('\', '/')
    return [string]::Equals($a, $b, [StringComparison]::OrdinalIgnoreCase)
}

function Publish-EnvironmentChange {
    # New processes read the registry directly, but already-running ones (most
    # importantly Explorer, the parent of every shell the user opens next) only
    # refresh when they get WM_SETTINGCHANGE.
    if (-not ('Wave.NativeMethods' -as [type])) {
        Add-Type -Namespace 'Wave' -Name 'NativeMethods' -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(
    System.IntPtr hWnd, uint Msg, System.IntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
    }

    $HWND_BROADCAST = [System.IntPtr] 0xffff
    $WM_SETTINGCHANGE = 0x1a
    $SMTO_ABORTIFHUNG = 0x2
    $result = [System.UIntPtr]::Zero
    [void] [Wave.NativeMethods]::SendMessageTimeout(
        $HWND_BROADCAST, $WM_SETTINGCHANGE, [System.IntPtr]::Zero, 'Environment',
        $SMTO_ABORTIFHUNG, 5000, [ref] $result)
}

$key = $null
try {
    $key = Open-EnvironmentKey -Scope $Scope

    $raw = $key.GetValue(
        'Path', '',
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)

    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($key.GetValueNames() -contains 'Path') { $kind = $key.GetValueKind('Path') }

    # Drop empty segments produced by stray or trailing semicolons.
    $entries = @($raw -split ';' | Where-Object { $_.Trim().Length -gt 0 })
    $present = @($entries | Where-Object { Test-SameDirectory $_ $Directory }).Count -gt 0

    switch ($Action) {
        'add' {
            if ($present) {
                Write-Output "PATH already contains '$Directory' ($Scope scope); nothing to do."
                exit 0
            }
            $entries = $entries + $Directory
        }
        'remove' {
            if (-not $present) {
                Write-Output "PATH does not contain '$Directory' ($Scope scope); nothing to do."
                exit 0
            }
            $entries = @($entries | Where-Object { -not (Test-SameDirectory $_ $Directory) })
        }
    }

    $key.SetValue('Path', ($entries -join ';'), $kind)
    Publish-EnvironmentChange
    Write-Output "PATH updated ($Action '$Directory', $Scope scope)."
    exit 0
}
catch {
    # Never fail the install over PATH: the app itself is already usable, and the
    # installer prints guidance for adding the directory by hand.
    Write-Output "Could not update PATH ($Action '$Directory', $Scope scope): $($_.Exception.Message)"
    exit 1
}
finally {
    if ($null -ne $key) { $key.Dispose() }
}
