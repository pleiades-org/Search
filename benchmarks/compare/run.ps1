[CmdletBinding()]
param(
    [string]$BaselineRevision = 'fb1d7969450357955e23c344912c7856478716db',
    [ValidateRange(1, 9)][int]$Runs = 3
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$artifactRoot = Join-Path $PSScriptRoot 'artifacts'
$resultRoot = Join-Path $PSScriptRoot 'results'
$baselineRoot = Join-Path $artifactRoot 'baseline-source'
$baselineHarness = Join-Path $baselineRoot 'benchmarks/compare'
$binarySuffix = if ([OperatingSystem]::IsWindows()) { '.exe' } else { '' }

function Invoke-CheckedTool {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}

function Get-Median {
    param([double[]]$Numbers)
    $sorted = @($Numbers | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 0) { return ($sorted[$middle - 1] + $sorted[$middle]) / 2 }
    return $sorted[$middle]
}

function Invoke-Measurement {
    param([string]$Executable, [string]$Label)
    Write-Output "Measuring $Label"
    $launch = @{
        FilePath = $Executable
        ArgumentList = @('run', 'fixtures', $Label)
        WorkingDirectory = $PSScriptRoot
        RedirectStandardOutput = (Join-Path $resultRoot "$Label-stdout.txt")
        RedirectStandardError = (Join-Path $resultRoot "$Label-stderr.txt")
        PassThru = $true
    }
    if ([OperatingSystem]::IsWindows()) { $launch.WindowStyle = 'Hidden' }
    $process = Start-Process @launch
    $observedPeak = 0L
    while (-not $process.HasExited) {
        $process.Refresh()
        $observedPeak = [Math]::Max($observedPeak, $process.PeakWorkingSet64)
        Start-Sleep -Milliseconds 10
    }
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw "$Label failed; inspect $Label-stderr.txt" }
    [pscustomobject]@{label=$Label; observed_peak_working_set_bytes=$observedPeak} |
        Export-Csv (Join-Path $resultRoot "$Label-resources.csv") -NoTypeInformation
    foreach ($kind in @('summary', 'samples', 'index')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot "fixtures/results/$Label-$kind.csv") `
            -Destination (Join-Path $resultRoot "$Label-$kind.csv")
    }
}

New-Item -ItemType Directory -Path $artifactRoot,$resultRoot,$baselineHarness -Force | Out-Null
$archive = Join-Path $artifactRoot 'baseline-source.tar'
Push-Location $repositoryRoot
try {
    Invoke-CheckedTool git @('archive', '--format=tar', "--output=$archive", $BaselineRevision)
    Invoke-CheckedTool tar @('-xf', $archive, '-C', $baselineRoot)
} finally { Pop-Location }
New-Item -ItemType Directory -Path (Join-Path $baselineHarness 'src') -Force | Out-Null
Copy-Item -Path (Join-Path $PSScriptRoot 'src/*') -Destination (Join-Path $baselineHarness 'src') -Recurse -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Cargo.toml') -Destination $baselineHarness -Force
Copy-Item -LiteralPath (Join-Path $baselineRoot 'Cargo.lock') -Destination $baselineHarness -Force
Invoke-CheckedTool cargo @('build', '--release', '--offline', '--manifest-path',
    (Join-Path $baselineHarness 'Cargo.toml'), '--target-dir', (Join-Path $artifactRoot 'baseline-target'))
Invoke-CheckedTool cargo @('build', '--release', '--offline', '--manifest-path', (Join-Path $PSScriptRoot 'Cargo.toml'))
$beforeBinary = Join-Path $artifactRoot "before$binarySuffix"
$afterBinary = Join-Path $artifactRoot "after$binarySuffix"
Copy-Item -LiteralPath (Join-Path $artifactRoot "baseline-target/release/search-comparison$binarySuffix") -Destination $beforeBinary -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "target/release/search-comparison$binarySuffix") -Destination $afterBinary -Force
Push-Location $PSScriptRoot
try {
    Invoke-CheckedTool $afterBinary @('prepare', 'fixtures')
    for ($run = 1; $run -le $Runs; $run++) {
        if ($run % 2 -eq 1) {
            Invoke-Measurement $beforeBinary "before-$run"
            Invoke-Measurement $afterBinary "after-$run"
        } else {
            Invoke-Measurement $afterBinary "after-$run"
            Invoke-Measurement $beforeBinary "before-$run"
        }
    }
} finally { Pop-Location }

$beforeRows = @(1..$Runs | ForEach-Object { Import-Csv (Join-Path $resultRoot "before-$_-summary.csv") })
$afterRows = @(1..$Runs | ForEach-Object { Import-Csv (Join-Path $resultRoot "after-$_-summary.csv") })
$comparison = foreach ($case in ($beforeRows.case | Select-Object -Unique)) {
    $before = @($beforeRows | Where-Object case -eq $case)
    $after = @($afterRows | Where-Object case -eq $case)
    $beforeMedian = Get-Median @($before.p50_us | ForEach-Object { [double]$_ })
    $afterMedian = Get-Median @($after.p50_us | ForEach-Object { [double]$_ })
    [pscustomobject]@{
        case=$case; runs=$Runs; matches=$before[0].matches
        before_p50_us=$beforeMedian; after_p50_us=$afterMedian
        before_p95_us=(Get-Median @($before.p95_us | ForEach-Object { [double]$_ }))
        after_p95_us=(Get-Median @($after.p95_us | ForEach-Object { [double]$_ }))
        speedup=($beforeMedian / $afterMedian)
        timer_limited=($afterMedian -lt 1)
    }
}
$comparison | Export-Csv (Join-Path $resultRoot 'comparison.csv') -NoTypeInformation
$manifest = Get-ChildItem -LiteralPath (Join-Path $repositoryRoot 'src'),(Join-Path $repositoryRoot 'tests'),(Join-Path $PSScriptRoot 'src') -Recurse -File |
    Sort-Object FullName | ForEach-Object { [pscustomobject]@{
        path=$_.FullName.Substring($repositoryRoot.Length + 1); sha256=(Get-FileHash -LiteralPath $_.FullName).Hash } }
$manifest | ConvertTo-Json | Set-Content (Join-Path $resultRoot 'source-manifest.json') -Encoding utf8NoBOM
$metadata = [ordered]@{
    date_utc=[DateTime]::UtcNow.ToString('o'); baseline_revision=$BaselineRevision
    compiler=(& rustc --version); os=[Environment]::OSVersion.VersionString
    logical_processors=[Environment]::ProcessorCount; runs=$Runs
    build='release defaults; default features disabled; same harness source and pinned library versions'
    before_sha256=(Get-FileHash $beforeBinary).Hash; after_sha256=(Get-FileHash $afterBinary).Hash
    caveats='Warm OS caches; no affinity/isolation; medians of run medians and run p95s; 10ms observed working-set peaks are lower bounds; first12 order changed from scheduling-dependent to source order.'
}
$metadata | ConvertTo-Json | Set-Content (Join-Path $resultRoot 'environment.json') -Encoding utf8NoBOM
$comparison | Format-Table case,before_p50_us,after_p50_us,before_p95_us,after_p95_us -AutoSize
