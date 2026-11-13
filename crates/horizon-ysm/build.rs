fn main() {
    cc::Build::new()
        .cpp(true)
        .file("src/cityhash/city.cc")
        .include("src/cityhash")
        .flag_if_supported("-std=c++11")
        .warnings(false)
        .compile("ysm_cityhash");
    println!("cargo:rerun-if-changed=src/cityhash/city.cc");
    println!("cargo:rerun-if-changed=src/cityhash/city.h");
}
