# Install son on Windows.
#
#   irm https://son-of-w.com/install.ps1 | iex
#
# It downloads son.exe from the GitHub release, checks it against the
# release's SHA256SUMS, puts it in %LOCALAPPDATA%\Programs\son, and adds
# that folder to your user PATH.
#
#   $env:SON_VERSION = "1.0.0"       install this version (default: the newest release)
#   $env:SON_INSTALL_DIR = "<dir>"   put son.exe here
#
# It does not run `son init`, does not change any agent's files, and does
# not touch your memory (%USERPROFILE%\.son-of-w). Run it again to upgrade.
# To remove son: `son uninstall`, then delete the folder.

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = "son-of-w/son"
$target = "x86_64-pc-windows-msvc"
$dir = if ($env:SON_INSTALL_DIR) { $env:SON_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\son" }

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "son needs 64-bit Windows."
}

$version = $env:SON_VERSION
if (-not $version) {
    try {
        $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers @{ "User-Agent" = "son-installer" }
    } catch {
        throw "Cannot reach GitHub to find the newest release. Set `$env:SON_VERSION and try again. ($_)"
    }
    $version = $latest.tag_name
}
$version = $version.TrimStart("v")
$tag = "v$version"
$archive = "son-$tag-$target.zip"
$base = "https://github.com/$repo/releases/download/$tag"

$work = Join-Path ([IO.Path]::GetTempPath()) ("son-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    Write-Host "Downloading son $version for $target"
    Invoke-WebRequest -Uri "$base/$archive" -OutFile (Join-Path $work $archive) -UseBasicParsing
    Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile (Join-Path $work "SHA256SUMS") -UseBasicParsing

    $want = $null
    foreach ($line in Get-Content (Join-Path $work "SHA256SUMS")) {
        $parts = $line -split "\s+", 2
        if ($parts.Count -eq 2 -and $parts[1].TrimStart("*") -eq $archive) { $want = $parts[0].ToLower() }
    }
    if (-not $want) { throw "SHA256SUMS of $tag does not list $archive." }
    $got = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $work $archive)).Hash.ToLower()
    if ($want -ne $got) {
        throw "The checksum of $archive is $got, but the release says $want. Nothing was installed."
    }

    Expand-Archive -Path (Join-Path $work $archive) -DestinationPath $work -Force
    $exe = Join-Path $work "son.exe"
    if (-not (Test-Path $exe)) { throw "$archive does not hold son.exe." }

    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    Copy-Item -Path $exe -Destination (Join-Path $dir "son.exe") -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

$installed = & (Join-Path $dir "son.exe") --version
Write-Host "Installed: $(Join-Path $dir 'son.exe') ($installed)"

# Add the folder to the user's PATH, once.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$onPath = ($userPath -split ";") | Where-Object { $_.TrimEnd("\") -ieq $dir.TrimEnd("\") }
if (-not $onPath) {
    $newPath = if ($userPath) { "$userPath;$dir" } else { $dir }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$env:Path;$dir"
    Write-Host "Added $dir to your PATH. Open a new terminal for other programs to see it."
}

Write-Host ""
Write-Host "Next: run 'son init' to set son up for your AI agents."
Write-Host "After an upgrade, run 'son init' again: it upgrades the database and the agent files."
Write-Host "Your memory in $env:USERPROFILE\.son-of-w was not touched."
