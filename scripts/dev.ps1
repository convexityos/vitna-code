# PowerShell helper script for Vitna Code development
[CmdletBinding()]
param (
    [Parameter(Position = 0)]
    [ValidateSet("check", "test", "lint", "fmt", "scan")]
    [string]$Action = "check"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

switch ($Action) {
    "check" {
        cargo check --workspace --all-targets
    }
    "test" {
        cargo test --workspace
    }
    "lint" {
        cargo clippy --workspace --all-targets -- -D warnings
    }
    "fmt" {
        cargo fmt --all
    }
    "scan" {
        Write-Host "Checking for forbidden em-dash characters..."
        $bad = Get-ChildItem -Path . -Recurse -File | Where-Object {
            $_.FullName -notmatch "\\.git\\" -and (Select-String -Path $_.FullName -Pattern "\u2014" -Quiet)
        }
        if ($bad) {
            Write-Error "Em-dash found in files: $($bad.FullName -join ', ')"
        } else {
            Write-Host "Em-dash check clean."
        }
    }
}
