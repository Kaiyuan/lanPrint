fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("ui/assets/lanprint-icon.ico")
            .set("ProductName", "lanPrint")
            .set("FileDescription", "lanPrint printer sharing")
            .set("LegalCopyright", "MIT License")
            .set("ProductVersion", env!("CARGO_PKG_VERSION"))
            .set("FileVersion", env!("CARGO_PKG_VERSION"));
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {err}");
        }
    }
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
}
