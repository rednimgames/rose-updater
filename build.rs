fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("res/client.ico");
        res.compile().unwrap();
    }

    println!("cargo:rerun-if-changed=build.rs");
}
