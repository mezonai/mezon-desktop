#requires -Version 7.0
param([switch]$CacheKeyOnly)

$ErrorActionPreference = "Stop"
if (-not $IsWindows) { throw "Run this script on Windows x64 with PowerShell 7." }
if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw "This ONNX Runtime build currently supports Windows x64 only."
}

$version = "1.22.0"
$revision = "f217402897f40ebba457e2421bc0a4702771968e"
$repo = Split-Path $PSScriptRoot -Parent
$package = Join-Path $repo "target\native\onnxruntime-mt"
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs) { throw "Visual Studio C++ x64 tools are required." }
$toolset = (Get-Content (Join-Path $vs "VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt") -Raw).Trim()
$recipeHash = (Get-FileHash $PSCommandPath -Algorithm SHA256).Hash.Substring(0, 16)
$cacheKey = "onnxruntime-$version-x64-mt-$toolset-$recipeHash"
if ($CacheKeyOnly) { Write-Output $cacheKey; return }

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}

$stamp = Join-Path $package "build-stamp.txt"
$ready = (Test-Path $stamp) -and ((Get-Content $stamp -Raw).Trim() -eq $cacheKey) -and
    (Test-Path (Join-Path $package "onnxruntime_session.lib"))
if (-not $ready) {
    $devShell = Join-Path $vs "Common7\Tools\Launch-VsDevShell.ps1"
    & $devShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
    Get-Command cl.exe, dumpbin.exe -CommandType Application -ErrorAction Stop | Out-Null
    $env:PATH = "$(Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja');$env:PATH"

    $source = Join-Path $repo "target\onnxruntime-source"
    if (-not (Test-Path $source)) {
        Invoke-Checked git @('clone', '--depth', '1', '--branch', "v$version", '--recursive',
            'https://github.com/microsoft/onnxruntime.git', $source)
    }
    $actualRevision = & git -C $source rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $actualRevision -ne $revision) { throw "Unexpected ONNX Runtime source revision." }

    $eigenRevision = "1d8b82b0740839c0de7f1242a3585e3390ff5f33"
    $eigen = Join-Path $repo "target\eigen-source"
    if (-not (Test-Path (Join-Path $eigen "Eigen\Core"))) {
        Invoke-Checked git @('init', $eigen)
        Invoke-Checked git @('-C', $eigen, 'fetch', '--depth', '1', 'https://gitlab.com/libeigen/eigen.git', $eigenRevision)
        Invoke-Checked git @('-C', $eigen, 'checkout', '--detach', $eigenRevision)
    }
    $actualEigenRevision = & git -C $eigen rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $actualEigenRevision -ne $eigenRevision) { throw "Unexpected Eigen source revision." }
    $eigenCmakePath = $eigen.Replace('\', '/')

    $build = Join-Path $repo "target\onnxruntime-build-$toolset"
    Invoke-Checked python @((Join-Path $source 'tools\ci_build\build.py'),
        '--build_dir', $build, '--config', 'Release', '--update', '--build', '--parallel', '2',
        '--skip_tests', '--enable_msvc_static_runtime', '--cmake_generator', 'Ninja',
        '--compile_no_warning_as_error', '--cmake_extra_defines',
        'onnxruntime_BUILD_SHARED_LIB=OFF', 'onnxruntime_BUILD_UNIT_TESTS=OFF', 'CMAKE_POLICY_VERSION_MINIMUM=3.5',
        "FETCHCONTENT_SOURCE_DIR_EIGEN3=$eigenCmakePath")

    $release = Join-Path $build "Release"
    $session = Join-Path $release "onnxruntime_session.lib"
    if (-not (Test-Path $session)) { throw "ONNX Runtime static libraries were not produced." }
    $directives = & dumpbin.exe /nologo /directives $session
    if ($LASTEXITCODE -ne 0 -or ($directives -match 'RuntimeLibrary=MD_') -or
        -not ($directives -match 'RuntimeLibrary=MT_StaticRelease')) {
        throw "ONNX Runtime was not built with the required static release CRT."
    }

    if (Test-Path $package) { Remove-Item $package -Recurse -Force }
    foreach ($lib in Get-ChildItem $release -Recurse -Filter '*.lib') {
        $directives = & dumpbin.exe /nologo /directives $lib.FullName
        if ($LASTEXITCODE -ne 0 -or ($directives -match 'RuntimeLibrary=MD_')) {
            throw "Unexpected dynamic CRT in $($lib.FullName)"
        }
        $destination = Join-Path $package ([IO.Path]::GetRelativePath($release, $lib.FullName))
        New-Item -ItemType Directory -Force (Split-Path $destination -Parent) | Out-Null
        Copy-Item $lib.FullName $destination
    }

    # ONNX Runtime 1.22 no longer builds re2, but ort-sys still emits `static=re2`.
    $re2 = Join-Path $package "_deps\re2-build"
    New-Item -ItemType Directory -Force $re2 | Out-Null
    $re2Source = Join-Path $re2 "re2_stub.c"
    Set-Content $re2Source "int mezon_re2_stub;"
    Invoke-Checked cl.exe @('/nologo', '/c', '/MT', '/Zl', "/Fo$(Join-Path $re2 're2_stub.obj')", $re2Source)
    Invoke-Checked lib.exe @('/nologo', "/OUT:$(Join-Path $re2 're2.lib')", (Join-Path $re2 're2_stub.obj'))
    Remove-Item $re2Source, (Join-Path $re2 're2_stub.obj')

    Set-Content $stamp $cacheKey
}

$env:ORT_LIB_LOCATION = $package
$env:ORT_LIB_PROFILE = ""
$env:ORT_PREFER_DYNAMIC_LINK = "0"
if ($env:GITHUB_ENV) {
    "ORT_LIB_LOCATION=$package", 'ORT_LIB_PROFILE=', 'ORT_PREFER_DYNAMIC_LINK=0' |
        Add-Content $env:GITHUB_ENV
}
Write-Host "ONNX Runtime $version ready (static CRT): $package"
