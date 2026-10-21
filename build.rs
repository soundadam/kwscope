fn main() {
    println!("cargo:rerun-if-changed=packaging/icon/kwscope.ico");
    // ort 的预编译 ONNX Runtime 里 CoreML 那部分用了 `@available`，要 clang 运行时库里的
    // `__isPlatformVersionAtLeast`；rustc 不链它，只好按 clang 的资源目录补上
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::process::Command::new("clang")
            .arg("--print-resource-dir")
            .output()
            .expect("run clang --print-resource-dir");
        let dir = String::from_utf8(out.stdout).expect("utf-8 path");
        println!("cargo:rustc-link-search=native={}/lib/darwin", dir.trim());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    // 资源编译器（rc.exe / windres）只在 Windows 主机上有；本仓不交叉编译 Windows
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("packaging/icon/kwscope.ico")
        .compile()
        .expect("compile Windows resources");
}
