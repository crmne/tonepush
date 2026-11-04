// Ruby supplies libruby's symbols when it loads the extension, so on macOS the
// library has to link with them left undefined. rb-sys asks for this in its own
// build script, but Cargo applies those link arguments to rb-sys alone. The gem
// leaves this file out: rb_sys/mkmf passes the same flag there.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
