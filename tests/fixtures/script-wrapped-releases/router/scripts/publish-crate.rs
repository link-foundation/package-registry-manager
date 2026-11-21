use std::process::Command;

fn main() {
    let mut cmd = Command::new("cargo");
    cmd.arg("publish").arg("--allow-dirty");
    cmd.output().unwrap();
}
