fn main() {
    println!("cargo:rerun-if-changed=packaging/icon/tonescope.ico");
    // 资源编译器（rc.exe / windres）只在 Windows 主机上有；本仓不交叉编译 Windows
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("packaging/icon/tonescope.ico")
        .compile()
        .expect("compile Windows resources");
}
