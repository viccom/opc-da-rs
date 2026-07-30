.PHONY: all debug release build test verify package package-win7 logs commit release-merge clean desktop-ui desktop-build desktop-run desktop-clean

all: debug

debug:
	cargo build

release:
	cargo build --release

build: release

# Quick test — for full quality gate use 'make verify'
test:
	cargo test

verify:
	pwsh -File scripts/verify.ps1

# ───── OPC DA Desktop (Tauri 2.0 GUI) ─────

# Build the React + TypeScript frontend only.
desktop-ui:
	cd opc-da-desktop/ui && npm install && npm run build

# Build the full standalone Windows GUI:
#   1. frontend (ui/dist)
#   2. release Rust binary (frontend assets are embedded via tauri::generate_context!)
# Output: target/release/opc-da-desktop.exe — double-click to launch.
desktop-build: desktop-ui
	cargo build --release -p opc-da-desktop

# Launch the GUI (must run from repo root so the embedded assets resolve).
desktop-run:
	target/release/opc-da-desktop.exe

desktop-clean:
	rm -rf opc-da-desktop/ui/dist opc-da-desktop/ui/node_modules
	cargo clean -p opc-da-desktop

# Creates a modern (Win10+) deployment zip via PowerShell single source of truth
package:
	pwsh -File scripts/package.ps1 -Task package

# Creates a Win7 / Server 2008 R2 legacy deployment zip
package-win7:
	pwsh -File scripts/package-win7.ps1

# Inspects application log file
logs:
	pwsh -File scripts/check-logs.ps1

# Runs quality gate, stages, commits, and pushes to remote
commit:
	pwsh -File scripts/commit.ps1 -Message "$(MSG)"

# Clean release merge from dev to main
release-merge:
	pwsh -File scripts/Merge-ToMain.ps1

clean:
	cargo clean
	rm -rf dist
