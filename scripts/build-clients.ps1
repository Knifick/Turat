[CmdletBinding()]
param(
    [switch]$SkipAndroidSignatureVerification,
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$artifactsDirectory = Join-Path $repositoryRoot 'artifacts'
$windowsPublishDirectory = Join-Path $artifactsDirectory 'publish\windows-native'
$androidPublishDirectory = Join-Path $artifactsDirectory 'publish\android-native'
$coreManifest = Join-Path $repositoryRoot 'client\core\Cargo.toml'
$coreDirectory = Split-Path -Parent $coreManifest
$windowsProject = Join-Path $repositoryRoot 'client\windows\TuratText.Windows.csproj'
$windowsIconScript = Join-Path $repositoryRoot 'scripts\build-windows-icon.ps1'
$windowsNativeDirectory = Join-Path $repositoryRoot 'client\windows\native'
$androidDirectory = Join-Path $repositoryRoot 'client\android'
$androidJniDirectory = Join-Path $androidDirectory 'app\src\main\jniLibs'
$keystorePath = Join-Path $repositoryRoot 'release-secrets\android\turattext-release.keystore'
$passwordPath = Join-Path $repositoryRoot 'release-secrets\android\android-signing-password.txt'
$windowsArtifact = Join-Path $artifactsDirectory 'Turat.exe'
$windowsZipArtifact = Join-Path $artifactsDirectory 'Turat-win-x64.zip'
$androidArtifact = Join-Path $artifactsDirectory 'Turat.apk'
$checksumsPath = Join-Path $artifactsDirectory 'SHA256SUMS.txt'
$windowsArtifactsDirectory = Join-Path $artifactsDirectory 'windows'
$androidArtifactsDirectory = Join-Path $artifactsDirectory 'android'

<#
.SYNOPSIS
Запускает внешнюю программу и судит об успехе только по коду возврата.

.DESCRIPTION
Windows PowerShell 5.1 превращает каждую строку stderr внешней программы в ошибку, а при
$ErrorActionPreference = 'Stop' это обрывает сборку на безобидных предупреждениях (например,
VsDevCmd.bat пишет в stderr, что не нашёл vswhere). Поэтому на время вызова возвращаем обычное
поведение и проверяем $LASTEXITCODE.
#>
function Invoke-Native {
    param([Parameter(Mandatory)][scriptblock]$Action)

    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Action
    }
    finally {
        $ErrorActionPreference = $previous
    }
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$Program,
        [Parameter(Mandatory)][string[]]$Arguments,
        [string]$DisplayName = $Program
    )

    Invoke-Native { & $Program @Arguments }
    if ($LASTEXITCODE -ne 0) {
        throw "$DisplayName завершился с кодом $LASTEXITCODE."
    }
}

function Get-AndroidBuildTool {
    param([Parameter(Mandatory)][string]$ToolName)

    $sdkDirectory = if ($env:ANDROID_SDK_ROOT) {
        $env:ANDROID_SDK_ROOT
    } elseif ($env:ANDROID_HOME) {
        $env:ANDROID_HOME
    } else {
        Join-Path $env:LOCALAPPDATA 'Android\Sdk'
    }
    $tool = Get-ChildItem -LiteralPath (Join-Path $sdkDirectory 'build-tools') -Filter $ToolName -Recurse -File -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($null -eq $tool) {
        throw "Не найден $ToolName в Android SDK."
    }
    return $tool.FullName
}

function Get-Adb {
    $adbCommand = Get-Command adb -ErrorAction SilentlyContinue
    if ($adbCommand) {
        return $adbCommand.Source
    }

    $sdkDirectory = if ($env:ANDROID_SDK_ROOT) {
        $env:ANDROID_SDK_ROOT
    } elseif ($env:ANDROID_HOME) {
        $env:ANDROID_HOME
    } else {
        Join-Path $env:LOCALAPPDATA 'Android\Sdk'
    }
    $adbPath = Join-Path $sdkDirectory 'platform-tools\adb.exe'
    if (Test-Path -LiteralPath $adbPath) {
        return $adbPath
    }

    return $null
}

function Install-AndroidArtifactIfConnected {
    param([Parameter(Mandatory)][string]$ApkPath)

    $adb = Get-Adb
    if (-not $adb) {
        Write-Host 'ADB не найден: автоматическая установка APK пропущена.' -ForegroundColor Yellow
        return
    }

    $deviceLines = Invoke-Native { & $adb devices }
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'Не удалось получить список ADB-устройств: автоматическая установка APK пропущена.' -ForegroundColor Yellow
        return
    }
    $deviceSerials = @($deviceLines | ForEach-Object {
        if ($_ -match '^([^\s]+)\s+device(?:\s|$)') {
            $Matches[1]
        }
    })
    if ($deviceSerials.Count -eq 0) {
        Write-Host 'Подключённый и авторизованный Android-телефон не найден: установка APK пропущена.'
        return
    }

    foreach ($deviceSerial in $deviceSerials) {
        Write-Host "Установка новой версии APK на устройство $deviceSerial..."
        Invoke-Checked -Program $adb -DisplayName "adb install ($deviceSerial)" -Arguments @(
            '-s', $deviceSerial, 'install', '-r', $ApkPath
        )
    }
}

function Invoke-MsvcCargo {
    param([Parameter(Mandatory)][string[]]$CargoArguments)

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw 'Для Windows Rust core необходимы Visual Studio Build Tools с workload C++.'
    }
    $installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ([string]::IsNullOrWhiteSpace($installation)) {
        throw 'Не найден MSVC C++ toolchain.'
    }
    $developerShell = Join-Path $installation 'Common7\Tools\VsDevCmd.bat'
    $escaped = $CargoArguments | ForEach-Object { '"' + $_.Replace('"', '\"') + '"' }
    $commandLine = 'call "' + $developerShell + '" -arch=x64 -host_arch=x64 && "' + $script:cargo + '" ' + ($escaped -join ' ')
    Invoke-Native { & $env:ComSpec /d /c $commandLine }
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') завершился с кодом $LASTEXITCODE."
    }
}

function Write-ClientChecksums {
    $existing = @{}
    if (Test-Path -LiteralPath $checksumsPath) {
        foreach ($line in Get-Content -LiteralPath $checksumsPath) {
            if ($line -match '^([0-9a-fA-F]{64})\s+(.+)$') {
                $existing[$Matches[2]] = $Matches[1].ToLowerInvariant()
            }
        }
    }
    # Не публикуем устаревшие клиентские имена после ребрендинга.
    @('TuratText.exe', 'TuratText-win-x64.zip', 'TuratText.apk') | ForEach-Object {
        $existing.Remove($_)
    }
    foreach ($item in @(
        @{ Name = 'Turat.exe'; Path = $windowsArtifact },
        @{ Name = 'Turat-win-x64.zip'; Path = $windowsZipArtifact },
        @{ Name = 'Turat.apk'; Path = $androidArtifact }
    )) {
        if (-not (Test-Path -LiteralPath $item.Path)) {
            # Иначе Get-FileHash вернёт $null, и падение выглядит как загадочное
            # «You cannot call a method on a null-valued expression» без имени файла.
            throw "Артефакт не найден: $($item.Path)"
        }
        $existing[$item.Name] = (Get-FileHash -LiteralPath $item.Path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $preferredOrder = @('Turat.exe', 'Turat-win-x64.zip', 'Turat.apk', 'turattext-server.jar', 'TuratText-VPS-Node.zip')
    $orderedNames = @($preferredOrder | Where-Object { $existing.ContainsKey($_) }) +
        @($existing.Keys | Where-Object { $_ -notin $preferredOrder } | Sort-Object)
    [System.IO.File]::WriteAllLines($checksumsPath, [string[]]@($orderedNames | ForEach-Object { "$($existing[$_])  $_" }))
}

# Скрипт должен запускаться и в Windows PowerShell 5.1, где нет оператора '?.'.
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { $null }
if (-not $cargo) {
    $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
}
if (-not (Test-Path -LiteralPath $cargo)) {
    throw 'Rust toolchain не найден. Установите stable Rust через rustup.'
}
$rustup = Join-Path (Split-Path -Parent $cargo) 'rustup.exe'
$gradle = Join-Path $androidDirectory 'gradlew.bat'
if (-not (Test-Path -LiteralPath $gradle)) {
    $gradle = (Get-Command gradle -ErrorAction Stop).Source
}

if (-not (Test-Path -LiteralPath $keystorePath) -or -not (Test-Path -LiteralPath $passwordPath)) {
    throw 'Не найдены release-keystore или пароль Android в release-secrets/android.'
}
$signingPassword = (Get-Content -LiteralPath $passwordPath -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($signingPassword)) {
    throw 'Файл пароля Android keystore пуст.'
}

New-Item -ItemType Directory -Force -Path @(
    $artifactsDirectory, $windowsPublishDirectory, $androidPublishDirectory,
    $windowsArtifactsDirectory, $androidArtifactsDirectory,
    $windowsNativeDirectory, $androidJniDirectory
) | Out-Null

Push-Location $repositoryRoot
try {
    Write-Host 'Проверка общего Rust core...'
    if (-not $SkipTests) {
        Invoke-MsvcCargo -CargoArguments @('test', '--manifest-path', $coreManifest)
    }

    Write-Host 'Сборка Rust core для Windows x64...'
    Invoke-MsvcCargo -CargoArguments @('build', '--manifest-path', $coreManifest, '--release')
    Copy-Item -LiteralPath (Join-Path $coreDirectory 'target\release\turattext_core.dll') `
        -Destination (Join-Path $windowsNativeDirectory 'turattext_core.dll') -Force

    Write-Host 'Подготовка многоразмерной Windows-иконки...'
    & $windowsIconScript

    Write-Host 'Сборка C# + XAML + WinUI 3...'
    Invoke-Checked -Program 'dotnet' -DisplayName 'dotnet publish WinUI 3' -Arguments @(
        'publish', $windowsProject, '-c', 'Release', '-r', 'win-x64', '--self-contained', 'true',
        '-p:WindowsAppSDKSelfContained=true', '-p:PublishSingleFile=true',
        '-p:IncludeNativeLibrariesForSelfExtract=true', '-p:IncludeAllContentForSelfExtract=true',
        '-p:DebugType=None', '-p:DebugSymbols=false', '-o', $windowsPublishDirectory
    )
    $publishedExe = Join-Path $windowsPublishDirectory 'Turat.exe'
    if (-not (Test-Path -LiteralPath $publishedExe)) {
        throw "Windows EXE не найден: $publishedExe"
    }
    Invoke-Checked -Program $publishedExe -DisplayName 'WinUI/Rust ABI smoke test' -Arguments @('--core-smoke')
    Copy-Item -LiteralPath $publishedExe -Destination $windowsArtifact -Force
    Copy-Item -LiteralPath $publishedExe -Destination (Join-Path $windowsArtifactsDirectory 'Turat.exe') -Force
    Compress-Archive -LiteralPath $windowsArtifact -DestinationPath $windowsZipArtifact -CompressionLevel Optimal -Force

    Write-Host 'Сборка Rust core для Android arm/arm64...'
    if (Test-Path -LiteralPath $rustup) {
        Invoke-Checked -Program $rustup -DisplayName 'rustup target add' -Arguments @(
            'target', 'add', 'aarch64-linux-android', 'armv7-linux-androideabi'
        )
    }
    if (-not $env:ANDROID_NDK_HOME) {
        $ndkRoot = Join-Path $env:LOCALAPPDATA 'Android\Sdk\ndk'
        $env:ANDROID_NDK_HOME = (Get-ChildItem -LiteralPath $ndkRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1).FullName
    }
    if (-not $env:ANDROID_SDK_ROOT) {
        $env:ANDROID_SDK_ROOT = Join-Path $env:LOCALAPPDATA 'Android\Sdk'
    }
    if (-not $env:ANDROID_HOME) {
        $env:ANDROID_HOME = $env:ANDROID_SDK_ROOT
    }
    Push-Location $coreDirectory
    try {
        Invoke-Checked -Program $cargo -DisplayName 'cargo ndk' -Arguments @(
            'ndk', '-t', 'arm64-v8a', '-t', 'armeabi-v7a', '-o', $androidJniDirectory,
            'build', '--release'
        )
    }
    finally {
        Pop-Location
    }

    Write-Host 'Сборка Kotlin + Jetpack Compose APK...'
    Push-Location $androidDirectory
    try {
        Invoke-Checked -Program $gradle -DisplayName 'Gradle Android release' -Arguments @(':app:assembleRelease', '--no-daemon')
    }
    finally {
        Pop-Location
    }
    $unsignedApk = Join-Path $androidDirectory 'app\build\outputs\apk\release\app-release-unsigned.apk'
    if (-not (Test-Path -LiteralPath $unsignedApk)) {
        throw "Unsigned APK не найден: $unsignedApk"
    }
    $alignedApk = Join-Path $androidPublishDirectory 'Turat-aligned.apk'
    $zipalign = Get-AndroidBuildTool -ToolName 'zipalign.exe'
    $apksigner = Get-AndroidBuildTool -ToolName 'apksigner.bat'
    Invoke-Checked -Program $zipalign -DisplayName 'zipalign' -Arguments @('-f', '4', $unsignedApk, $alignedApk)
    Invoke-Checked -Program $apksigner -DisplayName 'apksigner sign' -Arguments @(
        'sign', '--ks', $keystorePath, '--ks-key-alias', 'turattext',
        '--ks-pass', "pass:$signingPassword", '--key-pass', "pass:$signingPassword",
        '--out', $androidArtifact, $alignedApk
    )
    Copy-Item -LiteralPath $androidArtifact -Destination (Join-Path $androidArtifactsDirectory 'Turat.apk') -Force
    if (-not $SkipAndroidSignatureVerification) {
        Invoke-Checked -Program $apksigner -DisplayName 'apksigner verify' -Arguments @('verify', '--verbose', $androidArtifact)
    }

    Install-AndroidArtifactIfConnected -ApkPath $androidArtifact

    Write-ClientChecksums
    Write-Host "Готово: $windowsArtifact"
    Write-Host "Готово: $androidArtifact"
    Write-Host "Контрольные суммы: $checksumsPath"
}
finally {
    Pop-Location
}
