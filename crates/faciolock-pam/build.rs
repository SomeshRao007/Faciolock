fn main() {
    println!("cargo:rustc-link-arg=-l:libpam.so.0");
    println!("cargo:rerun-if-changed=build.rs");
}
