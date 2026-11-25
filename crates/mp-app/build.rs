//! 把图标和版本信息编进 exe，资源管理器里显示的就是它。
//! 工具链里没有资源编译器也不影响构建，只是 exe 用系统默认图标。

use std::env;
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/relay-icon.ico");
    let source = manifest.join("assets/icon.rc");
    let object = out.join("resource.o");
    for compiler in ["windres", "llvm-windres"] {
        let mut command = Command::new(compiler);
        command.current_dir(manifest.join("assets"));
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let status = command.arg("-i")
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .args(["-O", "coff"])
            .status();
        if matches!(status, Ok(status) if status.success()) {
            // 把编好的资源目标文件直接交给链接器。
            println!("cargo:rustc-link-arg-bins={}", object.display());
            return;
        }
    }
    println!("cargo:warning=没有找到资源编译器 windres，exe 将使用系统默认图标");
}
