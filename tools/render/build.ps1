param([Parameter(Mandatory = $true)][string]$Repo, [Parameter(Mandatory = $true)][string]$GateJson)
$ErrorActionPreference = 'Continue'
$gate = $GateJson | ConvertFrom-Json
if ($gate.kind -eq 'fifo') {
  $pending = New-Object System.Collections.ArrayList
  foreach ($line in Get-Content -LiteralPath $gate.path) {
    if ($line -match '^REQUEST (\S+) (\S+)') { $null = $pending.Add(@{lane=$Matches[1];stamp=$Matches[2]}) }
    elseif ($line -match '^RELEASE (\S+) (\S+)') {
      $lane=$Matches[1]; $stamp=$Matches[2]
      $index=-1
      for($i=0;$i -lt $pending.Count;$i++){if($pending[$i].lane -eq $lane -and $pending[$i].stamp -eq $stamp){$index=$i;break}}
      if($index -lt 0){for($i=0;$i -lt $pending.Count;$i++){if($pending[$i].lane -eq $lane){$index=$i;break}}}
      if($index -ge 0){$pending.RemoveAt($index)}
    }
  }
  if(!$pending.Count -or $pending[0].lane -ne $gate.lane -or $pending[0].stamp -ne $gate.stamp){throw 'Build requires ownership of the FIFO gate'}
} elseif ($gate.kind -eq 'local') {
  $lock = Get-Content -LiteralPath $gate.path -Raw | ConvertFrom-Json
  if($lock.marker -ne 'hm-render-lock-v1' -or $lock.owner.pid -ne $gate.owner.pid -or $lock.owner.started -ne $gate.owner.started){throw 'Build requires ownership of the local gate'}
} else { throw 'Unknown build gate' }
Set-Location -LiteralPath $Repo
$env:CARGO_BUILD_JOBS='1'
$env:CARGO_TARGET_DIR=Join-Path $env:LOCALAPPDATA 'hive-manager-target\loop1'
$cargoPath=Join-Path $Repo 'src-tauri\Cargo.toml'
$before=[IO.File]::ReadAllBytes($cargoPath)
$code=1
try {
  npm run tauri -- build --debug --no-bundle
  $code=$LASTEXITCODE
} finally {
  $after=[IO.File]::ReadAllBytes($cargoPath)
  $beforeText=[Text.Encoding]::UTF8.GetString($before).Replace("`r`n","`n")
  $afterText=[Text.Encoding]::UTF8.GetString($after).Replace("`r`n","`n")
  if($beforeText -ne $afterText){throw 'Tauri changed Cargo.toml content beyond line endings; review required'}
  if([Convert]::ToBase64String($before) -ne [Convert]::ToBase64String($after)){[IO.File]::WriteAllBytes($cargoPath,$before)}
  # Refresh stat metadata only; no source staging or content change.
  git update-index --refresh -- src-tauri/Cargo.toml
}
exit $code
