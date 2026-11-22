# Samples Bobby's processes every 2 s into a CSV: CPU time (ms), working set (MB), GPU memory (MB, dedicated).
# Usage: powershell -File sample.ps1 -Out run.csv -Seconds 600
param([string]$Out, [int]$Seconds = 600)
$names = 'bobby-app', 'llama-server', 'python', 'node'
'ts,name,pid,cpu_ms,ws_mb,vram_mb' | Set-Content $Out
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
  $ts = [DateTimeOffset]::Now.ToUnixTimeMilliseconds()
  $vram = @{}
  # per-process dedicated GPU memory: Windows counter (nvidia-smi says N/A per process under WDDM)
  (Get-Counter '\GPU Process Memory(*)\Dedicated Usage' -ErrorAction SilentlyContinue).CounterSamples | ForEach-Object {
    if ($_.InstanceName -match 'pid_(\d+)_') { $k = [int]$Matches[1]; $vram[$k] = [int]($vram[$k] + $_.CookedValue / 1MB) } }
  foreach ($p in Get-Process -Name $names -ErrorAction SilentlyContinue) {
    if ($p.Name -eq 'python' -and $p.Path -notmatch 'discord-bot') { continue }  # only Bobby's voice service
    $v = if ($vram.ContainsKey($p.Id)) { $vram[$p.Id] } else { 0 }
    '{0},{1},{2},{3:F0},{4:F0},{5}' -f $ts, $p.Name, $p.Id, $p.TotalProcessorTime.TotalMilliseconds, ($p.WorkingSet64 / 1MB), $v | Add-Content $Out
  }
  Start-Sleep -Seconds 2
}
