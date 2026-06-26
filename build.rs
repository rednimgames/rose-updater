use std::io;
#[cfg(windows)]
use winres::WindowsResource;

fn main() -> io::Result<()> {
    #[cfg(windows)]
    {
        WindowsResource::new()
            .set_icon("res/client.ico")
            .set_manifest_file("res/app.manifest")
            .compile()?;
    }
    Ok(())
}
