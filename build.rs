fn main() {
    // Unter Windows das Logo als Icon in die .exe einbetten.
    // Auf anderen Plattformen ist dieser Schritt nicht nötig.
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Rohteilcalc");
        res.set("FileDescription", "CNC Rohteilrechner für STEP-Dateien");
        res.compile()
            .expect("Windows-Ressourcen konnten nicht kompiliert werden");
    }
}
