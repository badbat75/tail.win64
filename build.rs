// Embeds a VERSIONINFO resource in tail.exe so Explorer and `Get-Command`
// show the crate version instead of 0.0.0.0. The fields come from Cargo.toml.

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
    if let Err(e) = res.compile() {
        panic!("cannot embed the version resource (is the Windows SDK installed?): {e}");
    }
}
