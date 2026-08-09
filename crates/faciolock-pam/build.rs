//! Link against libpam so the module carries a `DT_NEEDED` for it.
//!
//! A PAM module is `dlopen`ed into a process that already has libpam loaded, so
//! `pam_get_user` would usually resolve from the global scope even with no `DT_NEEDED` at
//! all. "Usually" is the problem: an application that loads libpam itself with `RTLD_LOCAL`
//! keeps those symbols out of the global scope, and the module then fails to load with an
//! undefined-symbol error at the lock screen. Naming the dependency costs nothing and
//! removes the case.
//!
//! `-l:libpam.so.0` rather than `-lpam` because `-lpam` needs the `libpam.so` development
//! symlink from `libpam0g-dev`, and the runtime `.so.0` is present on every machine that
//! has PAM at all. A Debian build has `libpam0g-dev` in Build-Depends and can use plain
//! `-lpam`; see wiki/07.
fn main() {
    println!("cargo:rustc-link-arg=-l:libpam.so.0");
    println!("cargo:rerun-if-changed=build.rs");
}
