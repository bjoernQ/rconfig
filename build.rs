fn main() {
    let host = std::env::var_os("HOST").expect("HOST not set");
    if let Some("windows") = host.to_str().unwrap().split('-').nth(2) {
        println!("cargo:rustc-cfg=host_os=\"windows\"");
    }
}
