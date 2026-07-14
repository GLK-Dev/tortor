use std::env;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("Images/tortor.ico");
        res.compile().unwrap();
    }
}
