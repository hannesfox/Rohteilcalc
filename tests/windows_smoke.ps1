# Lokaler Windows-11-Smoke-Test für die fertige Rohteilcalc-EXE.
# Prüft --diag, --render, den normalen GUI-Start und das Start-Log.
#
#   powershell -ExecutionPolicy Bypass -File tests\windows_smoke.ps1 `
#       -Exe dist\Rohteilcalc.exe -Step examples\ci_cube.step
#
# Hinweis: auf dem GitHub-Runner gibt es eine interaktive Sitzung, aber nur
# einen WARP-Softwareadapter. Dieser Test ist auf dem Zielrechner mit echter
# NVIDIA-/AMD-/Intel-Grafik auszuführen.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Step = "examples\ci_cube.step",
    [int]$WaitSeconds = 12
)
$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$tempOut = Join-Path $env:TEMP "rohteilcalc-smoke-$PID"
New-Item -ItemType Directory -Force -Path $tempOut | Out-Null

function Invoke-Checked([string[]]$CliArgs, [string]$Label) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exePath
    foreach ($a in $CliArgs) { [void]$psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(300000)) { $p.Kill(); throw "$Label: Timeout" }
    $text = $out.Result
    Write-Host "--- $Label (Exit $($p.ExitCode)) ---"
    Write-Host $text
    Write-Host $err.Result
    if ($p.ExitCode -ne 0) { throw "$Label: Exit-Code $($p.ExitCode)" }
    return $text
}

try {
    $report = Invoke-Checked @("--diag") "--diag"
    foreach ($needle in @("Betriebssystem:", "Ausgewähltes Backend:", "DX12-Shader-Compiler:", "Adapter", "DLL-Abhängigkeit: kernel32.dll")) {
        if (-not $report.Contains($needle)) { throw "--diag fehlt: $needle" }
    }

    $bmp = Join-Path $tempOut "smoke.bmp"
    [void](Invoke-Checked @("--render", $Step, $bmp) "--render")
    if (-not (Test-Path $bmp) -or (Get-Item $bmp).Length -lt 100000) { throw "--render: BMP fehlt oder zu klein" }

    $log = Join-Path $env:TEMP "Rohteilcalc.log"
    Remove-Item -Force $log -ErrorAction SilentlyContinue
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exePath
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = (Split-Path -Parent $exePath)
    $p = [System.Diagnostics.Process]::Start($psi)
    Start-Sleep -Seconds $WaitSeconds
    $p.Refresh()
    if ($p.HasExited) { throw "GUI ist während der Initialisierung beendet (Exit $($p.ExitCode))" }
    Write-Host "GUI läuft nach $WaitSeconds s (PID $($p.Id))"
    $startup = Get-Content -Raw $log -ErrorAction SilentlyContinue
    foreach ($needle in @("Grafik-Backend gewählt:", "CreationCallback", "Render-Pipelines installiert")) {
        if (-not $startup -or -not $startup.Contains($needle)) { throw "Start-Log fehlt: $needle" }
    }
    if ($startup -match "GPU-Fehler:|Pipeline-Fehler:|Panic:") { throw "GPU-Fehler im Start-Log" }
    [void]$p.CloseMainWindow()
    if (-not $p.WaitForExit(20000)) { $p.Kill(); throw "GUI schloss nicht" }
    Write-Host "PASS: --diag, --render und GUI-Start"
} finally {
    Remove-Item -Recurse -Force $tempOut -ErrorAction SilentlyContinue
}
