fn main() {
    // Linker scripts are provided by esp-hal via link-arg=-Tlinkall.x
    println!("cargo:rerun-if-changed=build.rs");
}
