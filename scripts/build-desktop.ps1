#!/usr/bin/env pwsh
# scripts/build-desktop.ps1
# ─────────────────────────────────────────
# One-shot build of the OPC DA Desktop GUI.
#
# Pipeline:
#   1. `npm install` + `npm run build`  →  opc-da-desktop/ui/dist/
#   2. `cargo build --release -p opc-da-desktop`  →  target/release/opc-da-desktop.exe
#
# Frontend assets are embedded into the exe at compile time
# (`tauri::generate_context!`), so the final exe is a self-contained
# distribution — no Node, no Vite, no source files required to run.
#
# Usage:
#   pwsh -File scripts/build-desktop.ps1            # full build
#   pwsh -File scripts/build-desktop.ps1 -Run       # build then launch
#   pwsh -File scripts/build-desktop.ps1 -SkipUi    # cargo-only (reuse existing dist/)

[CmdletBinding()]
param(
    [switch]$Run,
    [switch]$SkipUi,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

$dist = "opc-da-desktop/ui/dist"
$exe = "target/release/opc-da-desktop.exe"

if ($Clean) {
    Write-Host "Cleaning previous build artifacts..." -ForegroundColor Yellow
    Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
    cargo clean -p opc-da-desktop 2>&1 | Out-Null
}

if (-not $SkipUi) {
    Write-Host "==> [1/2] Building frontend (npm install + vite build)" -ForegroundColor Cyan
    Push-Location opc-da-desktop/ui
    if (-not (Test-Path "node_modules")) {
        npm install
        if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
    }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build failed" }
    Pop-Location
} else {
    Write-Host "==> [1/2] Skipping frontend (using existing $dist)" -ForegroundColor DarkGray
}

Write-Host "==> [2/2] Building release exe (frontend assets will be embedded)" -ForegroundColor Cyan
cargo build --release -p opc-da-desktop
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exeInfo = Get-Item $exe -ErrorAction SilentlyContinue
if (-not $exeInfo) {
    throw "Expected $exe not found after build"
}

Write-Host ""
Write-Host "✓ Built $exe ($([math]::Round($exeInfo.Length / 1MB, 2)) MB)" -ForegroundColor Green
Write-Host "  Double-click to launch — no installation required." -ForegroundColor Green
Write-Host ""

if ($Run) {
    Write-Host "Launching $exe ..." -ForegroundColor Cyan
    & $exeInfo.FullName
}