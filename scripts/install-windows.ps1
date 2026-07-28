$ErrorActionPreference = "Stop"

$BaseUrl = if ($env:MEZON_UPDATE_URL) { $env:MEZON_UPDATE_URL } else { "https://cdn.komu.vn/desktop/release/latest/" }
if (-not $BaseUrl.EndsWith("/")) { $BaseUrl += "/" }

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default { throw "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}

$manifestUrl = "${BaseUrl}latest-native-windows-${arch}.yml"
Write-Host "==> Fetching manifest $manifestUrl"
$manifest = (Invoke-WebRequest -UseBasicParsing -Uri $manifestUrl).Content

function Get-ManifestField([string]$body, [string]$key) {
    foreach ($line in $body -split "`n") {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith("${key}:")) {
            return $trimmed.Substring($key.Length + 1).Trim().Trim("'").Trim('"')
        }
    }
    return $null
}

$version = Get-ManifestField $manifest "version"
$path = Get-ManifestField $manifest "path"
$sha512 = Get-ManifestField $manifest "sha512"
if (-not $version -or -not $path -or -not $sha512) {
    throw "malformed manifest at $manifestUrl"
}

$tmp = Join-Path $env:TEMP "mezon-install-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp (Split-Path $path -Leaf)
    Write-Host "==> Downloading Mezon $version"
    Invoke-WebRequest -UseBasicParsing -Uri "${BaseUrl}${path}" -OutFile $zip

    Write-Host "==> Verifying checksum"
    $hashHex = (Get-FileHash -Path $zip -Algorithm SHA512).Hash
    $hashBytes = [byte[]]::new($hashHex.Length / 2)
    for ($i = 0; $i -lt $hashBytes.Length; $i++) {
        $hashBytes[$i] = [Convert]::ToByte($hashHex.Substring($i * 2, 2), 16)
    }
    $actual = [Convert]::ToBase64String($hashBytes)
    if ($actual -ne $sha512) {
        throw "checksum mismatch: expected $sha512, got $actual"
    }

    $extract = Join-Path $tmp "extract"
    Expand-Archive -Path $zip -DestinationPath $extract -Force
    $newExe = Get-ChildItem -Path $extract -Filter "mezon.exe" -Recurse | Select-Object -First 1
    if (-not $newExe) {
        throw "archive does not contain mezon.exe"
    }

    $installDir = Join-Path $env:LOCALAPPDATA "Programs\Mezon"
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    $target = Join-Path $installDir "mezon.exe"
    if (Test-Path $target) {
        $retired = Join-Path $installDir "mezon-old-$PID.exe"
        Remove-Item -Force -ErrorAction SilentlyContinue $retired
        Move-Item -Force $target $retired
    }
    Move-Item -Force $newExe.FullName $target
    Unblock-File -Path $target -ErrorAction SilentlyContinue

    $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    $shortcutPath = Join-Path $startMenu "Mezon.lnk"
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $target
    $shortcut.WorkingDirectory = $installDir
    $shortcut.Description = "Mezon desktop client"
    $shortcut.Save()

    $uninstallScript = Join-Path $installDir "uninstall.ps1"
    @'
$ErrorActionPreference = "SilentlyContinue"
Stop-Process -Name mezon -Force
Start-Sleep -Seconds 1
Remove-Item -Force (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Mezon.lnk")
Remove-Item -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mezon" -Recurse -Force
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $env:TEMP
Remove-Item -Recurse -Force $dir
Write-Host "Mezon has been uninstalled."
'@ | Set-Content -Path $uninstallScript -Encoding UTF8

    $regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mezon"
    New-Item -Path $regPath -Force | Out-Null
    Set-ItemProperty -Path $regPath -Name "DisplayName" -Value "Mezon"
    Set-ItemProperty -Path $regPath -Name "DisplayVersion" -Value $version
    Set-ItemProperty -Path $regPath -Name "Publisher" -Value "Mezon"
    Set-ItemProperty -Path $regPath -Name "InstallLocation" -Value $installDir
    Set-ItemProperty -Path $regPath -Name "DisplayIcon" -Value $target
    Set-ItemProperty -Path $regPath -Name "UninstallString" -Value "powershell.exe -ExecutionPolicy Bypass -File `"$uninstallScript`""
    Set-ItemProperty -Path $regPath -Name "NoModify" -Value 1 -Type DWord
    Set-ItemProperty -Path $regPath -Name "NoRepair" -Value 1 -Type DWord
    $sizeKB = [int]((Get-Item $target).Length / 1KB)
    Set-ItemProperty -Path $regPath -Name "EstimatedSize" -Value $sizeKB -Type DWord

    Write-Host "==> Installed Mezon $version to $target"
    Write-Host "Launch it from the Start Menu (Mezon). Uninstall from Settings > Apps."
}
finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}
