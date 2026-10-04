$ErrorActionPreference = 'Stop'
# Invoke-WebRequest renders a progress bar that is extremely slow when stdout is
# redirected (i.e. run non-interactively) on Windows PowerShell 5.1 — suppress
# it so the download doesn't appear to hang.
$ProgressPreference = 'SilentlyContinue'

$installDir = if ($env:RUNFILE_INSTALL_DIR) { $env:RUNFILE_INSTALL_DIR } else { "$env:LOCALAPPDATA\runfile\bin" }
# Version precedence: positional arg, then $env:RUNFILE_VERSION (the
# `iwr ... | iex` invocation form can't pass positional args), then latest.
$version = if ($args[0]) { "$($args[0])" } elseif ($env:RUNFILE_VERSION) { $env:RUNFILE_VERSION } else { 'latest' }

# The version goes into the download URL's path, so it has to be a release tag
# and nothing else: a `/` or a `..` in it reached somewhere other than these
# releases (audit SA-032). `\z` rather than `$`, which also matches before a
# final newline; `-cmatch`, because a tag's `v` is lowercase.
$tagPattern = '^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?\z'
if ($version -ne 'latest') {
  if ($version -cnotmatch $tagPattern) {
    throw "runfile: invalid version: $($version -replace '\p{Cc}', '') (expected latest, or a release tag such as v1.2.3)"
  }
  # Every release is tagged with the `v`, as `run :update` assumes too.
  if (-not $version.StartsWith('v')) { $version = "v$version" }
}

# Where releases come from. Gitea cuts every one; GitHub mirrors them when the
# mirror is pushed, so it can lag. `run :update --channel=github` sets this.
$channel = if ($env:RUNFILE_CHANNEL) { $env:RUNFILE_CHANNEL } else { 'gitea' }
$releases = switch ($channel) {
  'gitea'  { 'https://git.joaoverona.com/joaaoverona/runfile/releases' }
  'github' { 'https://github.com/JoaaoVerona/runfile/releases' }
  default  { throw "runfile: unknown channel: $channel (gitea or github)" }
}
# $server, not $host: $Host is an automatic variable, and assigning to it fails.
$server = ([Uri]$releases).Host

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86_64' }
  'ARM64' { 'aarch64' }
  default { throw "runfile: unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$target = "$arch-pc-windows-msvc"
$archive = "runfile-cli-$target.zip"

# Both hosts answer /releases/latest with a redirect to the newest release's tag
# page, so where it leads names the version, and the archive is then downloaded
# by name: /releases/download/<tag>/<asset> is the one shape the two share.
# Their own `latest` download aliases are not -- Gitea's is
# /releases/download/latest/<asset>, GitHub's /releases/latest/download/<asset>.
if ($version -eq 'latest') {
  $page = Invoke-WebRequest -Uri "$releases/latest" -UseBasicParsing
  # Where the redirect led: Windows PowerShell 5.1 and PowerShell 7 keep it in
  # different places.
  $final = if ($page.BaseResponse.ResponseUri) { $page.BaseResponse.ResponseUri } else { $page.BaseResponse.RequestMessage.RequestUri }
  if ("$final" -match '/releases/tag/([^/]+)$') { $version = $Matches[1] } else { throw "runfile: found no release on $server" }
  # The tag it names goes into a URL as well.
  if ($version -cnotmatch $tagPattern) { throw "runfile: $releases/latest led to $final, which is not a release" }
}

$url = "$releases/download/$version/$archive"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
  Write-Host "Downloading $archive ($version) from $server..."
  Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $archive) -UseBasicParsing
  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp

  New-Item -ItemType Directory -Path $installDir -Force | Out-Null
  $dest = Join-Path $installDir 'run.exe'

  # Windows won't let you overwrite a running .exe, but it WILL let you rename
  # one. Move any existing binary aside first so an in-place update (which runs
  # this script while the binary may be executing) works. Delete any stale .old
  # from a previous update first (it's a dead file by now and safe to remove).
  if (Test-Path $dest) {
    $old = "$dest.old"
    Remove-Item -Path $old -Force -ErrorAction SilentlyContinue
    Rename-Item -Path $dest -NewName 'run.exe.old' -ErrorAction SilentlyContinue
  }
  Move-Item -Path (Join-Path $tmp "runfile-cli-$target\run.exe") -Destination $dest -Force

  # Try to drop the .old now. Succeeds on a manual upgrade (the old binary
  # isn't running), so that path leaves no litter.
  $old = "$dest.old"
  if (Test-Path $old) { Remove-Item -Path $old -Force -ErrorAction SilentlyContinue }

  Write-Host "Installed run.exe $version to $dest"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not ($userPath -split ';' -contains $installDir)) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
    Write-Host ""
    Write-Host "Added $installDir to your PATH (open a new shell to use)."
  }
} finally {
  Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
