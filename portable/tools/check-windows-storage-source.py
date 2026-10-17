#!/usr/bin/env python3
"""Type/lint check Windows provider against cached Win32 declarations on Unix.

This does not build a Windows target, link Win32 symbols or execute any provider
function. Only std's OS-string/raw-handle glue is substituted for type checking.
An actual Windows toolchain/runtime remains a separate required gate.
"""

import json
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
root.mkdir()
(root / "src").mkdir()
source = Path(__file__).resolve().parents[1] / "crates/platform/src/windows.rs"
text = source.read_text()
for before, after in [
    ("#[cfg(windows)]\n", ""),
    ("use std::os::windows::ffi::OsStrExt;", "use crate::HostWide;"),
    ("use std::os::windows::io::FromRawHandle;", "use std::os::unix::io::FromRawFd;"),
    ("File::from_raw_handle(raw as *mut c_void)", "File::from_raw_fd(raw as i32)"),
    ("use std::os::windows::io::AsRawHandle;", "use std::os::unix::io::AsRawFd;"),
    ("file.as_raw_handle() as HANDLE", "file.as_raw_fd() as HANDLE"),
]:
    if before not in text:
        raise RuntimeError(f"Windows adapter source changed: {before}")
    text = text.replace(before, after)
(root / "src/windows.rs").write_text(text)
(root / "src/lib.rs").write_text("""#![allow(dead_code)]
mod windows;
#[derive(PartialEq)]
pub struct FileIdentity {pub volume:u64,pub object:u64}
fn child_name(_: &std::ffi::OsStr)->std::io::Result<()>{Ok(())}
trait HostWide {fn encode_wide(&self)->std::vec::IntoIter<u16>;}
impl HostWide for std::ffi::OsStr {
fn encode_wide(&self)->std::vec::IntoIter<u16>{
self.to_str().unwrap().encode_utf16().collect::<Vec<_>>().into_iter()}}
""")
features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Security_Authorization",
    "Win32_Storage_FileSystem",
    "Win32_System_Memory",
    "Win32_System_SystemServices",
    "Win32_System_Threading",
]
(root / "Cargo.toml").write_text(
    """[package]
name="soundroom-windows-storage-source-check"
version="0.0.0"
edition="2021"
[dependencies]
windows-sys={version="=0.36.1",features="""
    + json.dumps(features)
    + "}\n"
)
subprocess.run(
    ["cargo", "check", "--manifest-path", str(root / "Cargo.toml"), "--offline"], check=True
)
subprocess.run(
    [
        "cargo",
        "clippy",
        "--manifest-path",
        str(root / "Cargo.toml"),
        "--offline",
        "--locked",
        "--",
        "-D",
        "warnings",
    ],
    check=True,
)
print("PASS Win32 declaration/source type and lint check only; no Windows runtime claim")
