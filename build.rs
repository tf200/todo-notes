//! Embeds the icon and version details in the Windows exe. The taskbar, Start menu shortcut,
//! Explorer and Task Manager all read them from the file, not from the window.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/icon.ico");
        res.set("ProductName", "Todo & Notes");
        res.set("FileDescription", "Todo & Notes");
        res.set("LegalCopyright", "Copyright (c) 2026 Taha (tf200), MIT License");
        res.compile().expect("embedding the Windows icon and version info");
    }
}
