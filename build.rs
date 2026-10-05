// Embeds a VERSIONINFO resource in tail.exe so Explorer and `Get-Command`
// show the crate version instead of 0.0.0.0. The fields come from Cargo.toml.
//
// Also embeds an application manifest. tail draws no window, but the Windows
// App Certification Kit warns about any exe that declares no DPI awareness.

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    // The build script runs on the host: check the target, not cfg!(windows).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "tail for Windows")
        .set("ProductName", "tail-win")
        .set("OriginalFilename", "tail.exe")
        .set("InternalName", "tail");
    res.set_manifest(MANIFEST);
    if let Err(e) = res.compile() {
        panic!("cannot embed the version resource (is the Windows SDK installed?): {e}");
    }
}
