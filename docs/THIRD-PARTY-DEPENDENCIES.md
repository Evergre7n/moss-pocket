# Windows 发布包的第三方声明

Windows 发布包将 `THIRD-PARTY-NOTICES.md` 和 `licenses/` 放在 `MOSS-Pocket.exe` 同级目录。声明列出构建所用的 Cargo 锁定依赖，以及随二进制分发的 Opus、字体和原生运行库资料。清单由当前的 `Cargo.lock`、`mp-app` 依赖配置和发布工具链生成。

生成命令需要 Python 3.11 或更新版本、发布构建使用的 Windows GNU LLVM Rust 工具链、LLVM-MinGW 20260616 UCRT x86_64，以及 Cargo 缓存中的锁定源码归档。先运行 `cargo fetch --locked --target x86_64-pc-windows-gnullvm` 准备归档，然后在仓库根目录运行以下命令。输出目录应当尚未创建。

```powershell
python tools/generate_dependency_notices.py --llvm-mingw <LLVM-MinGW目录> --output target/windows-notices
```

如果 PATH 指向另一套工具链，可用 `--cargo` 和 `--rustc` 传入发布工具链的可执行文件路径。生成器使用 Cargo 的 Windows 普通和构建依赖图，同时核对所有源码归档的锁定 SHA-256。部分上游项目的许可证正文未随 crate 打包，版本固定的补充声明和出处保存在 [docs/licenses/dependencies](licenses/dependencies/sources.json)。

Symphonia 按 Mozilla Public License 2.0 分发。发布声明给出每个 Symphonia 组件对应的 crate 源码归档，和 Symphonia 0.5.5 的上游源码版本。Opus 由 opusic-sys 静态链接，声明保留 Opus 的版权、许可和专利公告。egui 内嵌字体使用随依赖提供的许可，设置中可用的 Windows 字体从本机系统目录读取。

源码仓库里的 `Cargo.lock` 还记录了其他平台和测试使用的包。Windows 发布声明以本次目标和构建配置为准。依赖版本、功能开关或工具链变动时，对照新构建重新生成声明并检查新增许可。
