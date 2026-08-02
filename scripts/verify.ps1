<#
.SYNOPSIS
    Universal Quality Gate for opc-cli.
.DESCRIPTION
    Runs cargo fmt, clippy, doc tests, and standard workspace tests.
    Halts execution strictly on any non-zero exit code.
    Reports What/Where/Why on failure for human and AI diagnostics.
.PARAMETER Verbose
    When set, captures cargo output and replays the last 20 lines on failure.
#>

param(
    [switch]$Verbose
)

$ErrorActionPreference = 'Stop'
$ErrorView = 'NormalView'

# Temp log for -Verbose stderr capture
$script:LogFile = [System.IO.Path]::GetTempFileName()

function Invoke-Gate {
    param(
        [string]$GateName,
        [string]$Command
    )

    Write-Host "`n>>> $GateName" -ForegroundColor Yellow

    if ($Verbose) {
        Invoke-Expression "$Command 2>&1" | Tee-Object -FilePath $script:LogFile
    } else {
        Invoke-Expression $Command
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n========================================" -ForegroundColor Red
        Write-Host " VERIFICATION FAILED" -ForegroundColor Red
        Write-Host "========================================" -ForegroundColor Red
        Write-Host " What : $GateName" -ForegroundColor Red
        Write-Host " Where: $Command" -ForegroundColor Red
        Write-Host " Why  : Process exited with code $LASTEXITCODE" -ForegroundColor Red

        if ($Verbose -and (Test-Path $script:LogFile)) {
            Write-Host " Hint : Last 20 lines of output:" -ForegroundColor Red
            Get-Content $script:LogFile -Tail 20 | ForEach-Object {
                Write-Host "   $_" -ForegroundColor DarkRed
            }
        }

        Write-Host "========================================`n" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Write-Host "Running Verification Pipeline..." -ForegroundColor Cyan

Invoke-Gate -GateName "Formatter Check" -Command "cargo fmt --all -- --check"
Invoke-Gate -GateName "Linter Check" -Command "cargo clippy --workspace --all-targets --all-features -- -D warnings"
Invoke-Gate -GateName "Doc Compilation Check" -Command "cargo test --doc --workspace"
Invoke-Gate -GateName "Unit & Integration Tests" -Command "cargo test --workspace"

# Polyfill Compilation Gate — compat/ crates excluded from workspace,
# so they must be compiled independently to catch breakage.
$compatDir = Join-Path $PSScriptRoot ".." "compat"
if (Test-Path $compatDir) {
    $polyfillManifests = @(Get-ChildItem -Path $compatDir -Filter "Cargo.toml" -Recurse -Depth 1)
    foreach ($manifest in $polyfillManifests) {
        $crateName = (Split-Path -Parent $manifest.FullName | Split-Path -Leaf)
        Invoke-Gate -GateName "Polyfill Build: $crateName" -Command "cargo build --manifest-path `"$($manifest.FullName)`" --release"
    }
}

# End-to-End Gate — opc-da-client-test 对自建 server 跑 13 探针（Windows COM job，阶段 4 硬化）。
# 释放旧 server exe → 重 build → /RegServer（双视图注册）→ 跑 client-test（exit 0 = 13 passed）→ 清理。
$serverExe = Join-Path $PSScriptRoot ".." "target" "debug" "opc-da-server.exe"
taskkill /F /IM opc-da-server.exe 2>$null | Out-Null
Invoke-Gate -GateName "End-to-End Server Build" -Command "cargo build -p opc-da-server"
$regCommand = "& '" + $serverExe + "' '/RegServer'"
Invoke-Gate -GateName "End-to-End Register (/RegServer)" -Command $regCommand
Invoke-Gate -GateName "End-to-End Client Test (13 probes)" -Command "cargo run -p opc-da-client-test"
taskkill /F /IM opc-da-server.exe 2>$null | Out-Null

# Cleanup temp log
if (Test-Path $script:LogFile) { Remove-Item $script:LogFile -ErrorAction SilentlyContinue }

Write-Host "`nAll Gates Passed! ✅" -ForegroundColor Green
exit 0
