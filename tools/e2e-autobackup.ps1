$ErrorActionPreference = "Stop"
$app      = Join-Path $PSScriptRoot "..\src-tauri\target\release\saveward.exe"
$cfgDir   = Join-Path $env:APPDATA "Saveward"
$settings = Join-Path $cfgDir "settings.json"
$saved    = Join-Path $env:TEMP "saveward-settings-before-test.json"
$work     = Join-Path $env:TEMP "saveward-e2e"
$name     = "Saveward Test Game"

Get-Process saveward -ErrorAction SilentlyContinue | Stop-Process
$hadSettings = Test-Path $settings
if ($hadSettings) { Copy-Item $settings $saved -Force }
else {
    [pscustomobject]@{ backup_dir = (Join-Path $cfgDir "backups"); versions_to_keep = 10; auto_backup = $true;
                       show_covers = $true; start_with_windows = $false; games = [pscustomobject]@{} } |
        ConvertTo-Json -Depth 10 | Set-Content $settings
}
try {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory "$work\game", "$work\saves" | Out-Null
    Copy-Item "$env:WINDIR\System32\PING.EXE" "$work\game\testgame.exe"
    Set-Content "$work\saves\slot1.sav" "level 7"

    $s = Get-Content $settings -Raw | ConvertFrom-Json
    $s.auto_backup = $true
    $s | Add-Member -NotePropertyName cloud -NotePropertyValue $null -Force
    $s | Add-Member -NotePropertyName show_popup -NotePropertyValue $true -Force
    $entry = [pscustomobject]@{ extra_paths = @("$work\saves"); excluded_paths = @(); custom = $true;
                                exe = "$work\game\testgame.exe"; ignored = $false; cover = $null }
    $s.games | Add-Member -NotePropertyName $name -NotePropertyValue $entry -Force
    $s | ConvertTo-Json -Depth 10 | Set-Content $settings

    $p = Start-Process $app -ArgumentList "--background" -PassThru
    Start-Sleep 5
    $bg = Get-Process -Id $p.Id
    $webviews = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -like "msedgewebview2*" -and $_.ParentProcessId -eq $p.Id }).Count
    "background mode: app {0:N0} MB, webview processes: {1}" -f ($bg.WorkingSet64 / 1MB), $webviews

    "playing the fake game for ~5 s..."
    Start-Process "$work\game\testgame.exe" -ArgumentList "-n 6 127.0.0.1" -WindowStyle Hidden -Wait
    "game closed; waiting for the auto-backup..."
    $index = Join-Path $s.backup_dir "$name\index.json"
    $deadline = (Get-Date).AddSeconds(30)
    $popupSeen = $false
    while (-not (Test-Path $index) -and (Get-Date) -lt $deadline) {
        Start-Sleep 1
        if (@(Get-CimInstance Win32_Process | Where-Object { $_.Name -like "msedgewebview2*" -and $_.ParentProcessId -eq $p.Id }).Count -gt 0) { $popupSeen = $true }
    }
    for ($i = 0; $i -lt 6 -and -not $popupSeen; $i++) {
        Start-Sleep 1
        if (@(Get-CimInstance Win32_Process | Where-Object { $_.Name -like "msedgewebview2*" -and $_.ParentProcessId -eq $p.Id }).Count -gt 0) { $popupSeen = $true }
    }
    "pop-up appeared: $popupSeen"
    Start-Sleep 12
    $left = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -like "msedgewebview2*" -and $_.ParentProcessId -eq $p.Id }).Count
    "pop-up closed itself (webview processes left: $left): $($left -eq 0)"
    if (Test-Path $index) {
        $v = Get-Content $index -Raw | ConvertFrom-Json
        "PASS: auto-backup created: $($v[0].file), $($v[0].files) file(s), note '$($v[0].note)'"
    } else { "FAIL: no backup within 30 s of the game closing" }

    $bg.Refresh(); "after: app {0:N0} MB" -f ($bg.WorkingSet64 / 1MB)
}
finally {
    Get-Process saveward -ErrorAction SilentlyContinue | Stop-Process
    if ($hadSettings) { Copy-Item $saved $settings -Force } else { Remove-Item $settings -Force }
    Remove-Item (Join-Path $s.backup_dir $name) -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    "cleaned up; your settings were restored"
}
