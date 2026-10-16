# Run on a Windows host/runner with a Rust MSVC toolchain. No audio devices/UI open.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$manifest = Join-Path $PSScriptRoot '..\Cargo.toml'
function Invoke-CheckedCargo([string[]] $Arguments) {
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo qualification failed ($LASTEXITCODE)" }
}
Invoke-CheckedCargo -Arguments @('check','--locked','--manifest-path',$manifest,'--workspace','--all-targets')
Invoke-CheckedCargo -Arguments @('test','--locked','--manifest-path',$manifest,'--workspace')
Invoke-CheckedCargo -Arguments @('clippy','--locked','--manifest-path',$manifest,'--workspace','--all-targets','--','-D','warnings')
# Includes owner/protected DACL, hard-link rejection, exclusive sharing lock, native
# Schannel WSS certificate/hostname/Origin and actual import/project/export workflows.
# Hardware input/output, packaging/signing and production dependency review remain gates.
