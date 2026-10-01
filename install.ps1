# Installs the `annox` binary from a GitHub release on Windows, and optionally the agent skill.
#
#   powershell -ExecutionPolicy ByPass -c "irm https://gaardhus.github.io/annox/install.ps1 | iex"
#   powershell -ExecutionPolicy ByPass -c "& ([scriptblock]::Create((irm https://gaardhus.github.io/annox/install.ps1))) -Skill"
#
# Options (or environment variables, which also apply through a plain `irm | iex`):
#   -Version vX.Y.Z   ANNOX_VERSION             release to install (default: the latest)
#   -Dir DIR          ANNOX_INSTALL_DIR         where to put `annox.exe` (default: ~\.local\bin)
#   -Skill            ANNOX_SKILL=1             also install the skill for Claude Code
#   -SkillDir DIR     ANNOX_SKILL_DIR           where skills go (default: ~\.claude\skills)
#   -NoModifyPath     ANNOX_NO_MODIFY_PATH=1    don't add the install directory to the user PATH

param(
  [string]$Version = $env:ANNOX_VERSION,
  [string]$Dir = $env:ANNOX_INSTALL_DIR,
  [switch]$Skill,
  [string]$SkillDir = $env:ANNOX_SKILL_DIR,
  [switch]$NoModifyPath,
  [switch]$Help
)

# Everything runs in a function so that `irm | iex` leaves no variables or preferences behind in
# the caller's session, and errors `throw` instead of `exit`, which would close it.
function Install-Annox {
  param($Version, $Dir, $Skill, $SkillDir, $NoModifyPath)

  $ErrorActionPreference = 'Stop'
  # Invoke-WebRequest's progress bar slows downloads to a crawl in Windows PowerShell.
  $ProgressPreference = 'SilentlyContinue'
  # Windows PowerShell 5.1 may not offer TLS 1.2, which GitHub requires, by default.
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $repo = 'gaardhus/annox'
  $target = 'x86_64-pc-windows-msvc'
  function Say($msg) { [Console]::Error.WriteLine("annox: $msg") }

  if (-not [Environment]::Is64BitOperatingSystem) {
    throw "annox: no prebuilt binary for 32-bit Windows; build from source with: cargo install --git https://github.com/$repo annox-lsp"
  }
  # ARM64 Windows runs the x64 binary under emulation.

  if (-not $Version) {
    try {
      $Version = (Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases/latest").tag_name
    } catch {
      throw "annox: could not find the latest release: $_"
    }
  }
  if (-not $Version.StartsWith('v')) { $Version = "v$Version" }

  $name = "annox-$Version-$target"
  $base = "https://github.com/$repo/releases/download/$Version"
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
  New-Item -ItemType Directory -Path $tmp | Out-Null
  try {
    Say "downloading $name"
    $zip = Join-Path $tmp "$name.zip"
    $sums = Join-Path $tmp 'SHA256SUMS'
    try {
      Invoke-WebRequest -UseBasicParsing -OutFile $zip "$base/$name.zip"
    } catch {
      throw "annox: could not download $base/$name.zip: $_"
    }
    try {
      Invoke-WebRequest -UseBasicParsing -OutFile $sums "$base/SHA256SUMS"
    } catch {
      throw "annox: could not download $base/SHA256SUMS: $_"
    }

    $pattern = "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape("$name.zip"))$"
    $match = Get-Content $sums | Select-String -Pattern $pattern | Select-Object -First 1
    if (-not $match) { throw "annox: $name.zip is not in SHA256SUMS" }
    $expected = $match.Matches[0].Groups[1].Value
    $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash
    if ($actual -ne $expected) { throw "annox: checksum mismatch for $name.zip" }

    Expand-Archive -Path $zip -DestinationPath $tmp
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $exe = Join-Path $Dir 'annox.exe'
    # A running exe can't be overwritten, but it can be renamed, so this works on an annox that is
    # in use, including the one running `annox update`.
    if (Test-Path $exe) {
      Remove-Item -Force "$exe.old" -ErrorAction SilentlyContinue
      Move-Item -Force $exe "$exe.old"
    }
    Copy-Item (Join-Path $tmp "$name\annox.exe") $exe
    Remove-Item -Force "$exe.old" -ErrorAction SilentlyContinue
    Say "installed $exe ($Version)"

    if ($Skill) {
      $src = Join-Path $tmp "$name\skills\annox"
      if (-not (Test-Path $src)) { throw "annox: $Version has no skill in its release archive" }
      New-Item -ItemType Directory -Force -Path $SkillDir | Out-Null
      $dest = Join-Path $SkillDir 'annox'
      if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
      Copy-Item -Recurse $src $dest
      Say "installed the skill in $dest"
    }
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }

  $full = (Resolve-Path $Dir).Path.TrimEnd('\')
  $onPath = { param($p) ($p -split ';') | Where-Object { $_ -and $_.TrimEnd('\') -ieq $full } }
  if (& $onPath $env:Path) { return }
  if ($NoModifyPath) {
    Say "$full is not on your PATH; add it to use ``annox``"
    return
  }
  # Read and write the registry rather than [Environment]::SetEnvironmentVariable, which would
  # expand entries like %USERPROFILE% in the user's PATH for good.
  $key = Get-Item 'HKCU:\Environment'
  $userPath = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
  if (-not (& $onPath $userPath)) {
    $userPath = (@($full) + ($userPath -split ';' | Where-Object { $_ })) -join ';'
    Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value $userPath -Type ExpandString
    # Setting any variable this way tells open programs, like Explorer, that the environment
    # changed, so new terminals pick up the PATH.
    [Environment]::SetEnvironmentVariable('ANNOX_INSTALL_PATH_REFRESH', '1', 'User')
    [Environment]::SetEnvironmentVariable('ANNOX_INSTALL_PATH_REFRESH', $null, 'User')
  }
  $env:Path = "$full;$env:Path"
  Say "added $full to your PATH; restart your terminal to use ``annox`` there"
}

if ($Help) {
  @'
Usage: install.ps1 [-Version vX.Y.Z] [-Dir DIR] [-Skill] [-SkillDir DIR] [-NoModifyPath]

  -Version vX.Y.Z  release to install (default: the latest; or ANNOX_VERSION)
  -Dir DIR         where to put `annox.exe` (default: ~\.local\bin; or ANNOX_INSTALL_DIR)
  -Skill           also install the skill for Claude Code (or ANNOX_SKILL=1)
  -SkillDir DIR    where skills go (default: ~\.claude\skills; or ANNOX_SKILL_DIR)
  -NoModifyPath    don't add the install directory to the user PATH (or ANNOX_NO_MODIFY_PATH=1)
'@
  return
}

Install-Annox `
  -Version $Version `
  -Dir $(if ($Dir) { $Dir } else { Join-Path $HOME '.local\bin' }) `
  -Skill ($Skill -or $env:ANNOX_SKILL -eq '1') `
  -SkillDir $(if ($SkillDir) { $SkillDir } else { Join-Path $HOME '.claude\skills' }) `
  -NoModifyPath ($NoModifyPath -or $env:ANNOX_NO_MODIFY_PATH -eq '1')
