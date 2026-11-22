$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

try {
    switch ($env:POSTINSTALL_STAGE) {
        'check' {
            $version = [version] $env:POSTINSTALL_VERSION
            $packages = Dism\Get-AppxProvisionedPackage -Online | Where-Object {
                $_.DisplayName -eq 'Microsoft.DesktopAppInstaller' -and [version] $_.Version -ge $version
            }
            if ($packages) {
                'current'
            } else {
                'needs-install'
            }
        }
        'install' {
            $dependencies = $env:POSTINSTALL_DEPENDENCIES.Split('|')
            Dism\Add-AppxProvisionedPackage -Online `
                -PackagePath $env:POSTINSTALL_BUNDLE `
                -LicensePath $env:POSTINSTALL_LICENSE `
                -DependencyPackagePath $dependencies | Out-Null
        }
        'register' {
            Appx\Add-AppxPackage -RegisterByFamilyName -MainPackage Microsoft.DesktopAppInstaller_8wekyb3d8bbwe
        }
        default {
            throw 'Unknown App Installer operation.'
        }
    }
} catch {
    [Console]::Error.WriteLine($_.ToString())
    exit 1
}
