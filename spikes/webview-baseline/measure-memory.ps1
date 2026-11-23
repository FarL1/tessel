param([int]$rootPid, [string]$label)
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, PrivatePageCount
$ids = New-Object System.Collections.Generic.HashSet[int]; [void]$ids.Add($rootPid)
do { $added = $false; foreach ($p in $all) { if ($ids.Contains([int]$p.ParentProcessId) -and $ids.Add([int]$p.ProcessId)) { $added = $true } } } while ($added)
$perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $ids.Contains([int]$_.IDProcess) }
$pws = ($perf | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB
$ws = ($perf | Measure-Object WorkingSet -Sum).Sum / 1MB
$commit = ($all | Where-Object { $ids.Contains([int]$_.ProcessId) } | Measure-Object PrivatePageCount -Sum).Sum / 1MB
"{0,-34} 进程数 {1,2} · 私有工作集 {2,6:N1}MB · 工作集 {3,6:N1}MB · 提交 {4,6:N1}MB" -f $label, $ids.Count, $pws, $ws, $commit
