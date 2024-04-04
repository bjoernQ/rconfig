use std::path::PathBuf;

pub fn main() {
    rconfig::apply_config(&PathBuf::from("./config/rconfig.toml"));
}
