use chrono::{Datelike, Local};
use std::{
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
};

extern crate winres;
fn main() {
    setup_enhanced();
    let now = Local::now();
    let copyright = format!("Copyright © {} REVENGE977 & Smart Code OOD", now.year());
    let pkg_name = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "stremio-enhanced-ng".to_string());
    let pkg_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "5.0.27".to_string());
    let exe_name = format!("{pkg_name}.exe");
    let mut res = winres::WindowsResource::new();
    res.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    <assemblyIdentity
        type="win32"
        name="stremio-enhanced-ng"
        version="5.0.27.0"
        processorArchitecture="*"
    />
    <dependency>
        <dependentAssembly>
            <assemblyIdentity
                type="win32"
                name="Microsoft.Windows.Common-Controls"
                version="6.0.0.0"
                processorArchitecture="*"
                publicKeyToken="6595b64144ccf1df"
                language="*"
            />
        </dependentAssembly>
    </dependency>
    <application xmlns="urn:schemas-microsoft-com:asm.v3">
        <windowsSettings>
            <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
            <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
            <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
        </windowsSettings>
    </application>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <!-- Windows 10 and Windows 11 -->
            <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
            <!-- Windows 8.1 -->
            <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
            <!-- Windows 8 -->
            <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
            <!-- Windows 7 -->
            <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
        </application>
    </compatibility>
    <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
        <security>
            <requestedPrivileges>
                <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
            </requestedPrivileges>
        </security>
    </trustInfo>
</assembly>"#,
    );
    res.set("FileDescription", "Stremio Enhanced - Freedom to Stream");
    res.set("ProductName", "Stremio Enhanced");
    res.set("ProductVersion", &pkg_version);
    res.set("FileVersion", &pkg_version);
    res.set("CompanyName", "Smart Code OOD & REVENGE977");
    res.set("InternalName", "stremio-enhanced-ng");
    res.set("LegalCopyright", &copyright);
    res.set("OriginalFilename", &exe_name);
    res.set_icon_with_id("images/stremio.ico", "MAINICON");
    res.append_rc_content(r##"SPLASHIMAGE IMAGE "images/stremio.png""##);
    res.compile().unwrap();

    //extract libmpv-2
    let target = std::env::var("TARGET").unwrap();
    let (arch, archive, flags) = match target.as_str() {
        "x86_64-pc-windows-msvc" => ("x64", "libmpv-2_x64.zip", "/LIBPATH:.\\mpv-x64"),
        "aarch64-pc-windows-msvc" => ("arm64", "libmpv-2_arm64.zip", "/LIBPATH:.\\mpv-arm64"),
        _ => panic!("Unsupported target {}", target),
    };
    println!("cargo:rustc-env=ARCH={}", arch);
    println!("cargo:rustc-link-arg={}", flags);
    let archive = fs::read(archive).unwrap();
    let target_dir = PathBuf::from(".");
    zip_extract::extract(Cursor::new(archive), &target_dir, true).ok();
}

fn watch_dir_recursively(dir: &Path) {
    if !dir.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", dir.display());
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                watch_dir_recursively(&path);
            } else {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn setup_enhanced() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let enhanced_dir = manifest_dir.join("src").join("stremio_app").join("enhanced");
    let enhanced_src = enhanced_dir.join("src");
    let enhanced_build_js = enhanced_dir.join("build.js");
    let enhanced_package_json = enhanced_dir.join("package.json");
    let enhanced_tsconfig = enhanced_dir.join("tsconfig.json");
    let enhanced_output = enhanced_dir.join("enhanced.js");

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    watch_dir_recursively(&enhanced_src);
    println!("cargo:rerun-if-changed={}", enhanced_build_js.display());
    println!("cargo:rerun-if-changed={}", enhanced_package_json.display());
    println!("cargo:rerun-if-changed={}", enhanced_tsconfig.display());

    let status = Command::new("node")
        .arg(&enhanced_build_js)
        .current_dir(&enhanced_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            if !enhanced_output.exists() {
                panic!(
                    "Failed to compile enhanced.js (node exit code: {:?}). Ensure Node.js and npm/npx are installed.",
                    s.code()
                );
            } else {
                eprintln!("Warning: Failed to rebuild enhanced.js; using existing bundle.");
            }
        }
        Err(e) => {
            if !enhanced_output.exists() {
                panic!(
                    "Failed to run 'node' ({}) to compile enhanced.js, and no existing bundle was found. Please install Node.js.",
                    e
                );
            } else {
                eprintln!("Warning: Node.js not found ({e}); using existing enhanced.js.");
            }
        }
    }
}
