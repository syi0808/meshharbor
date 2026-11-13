fn main() {
    println!("cargo:rerun-if-changed=src/atlas_wrapper.cpp");
    println!("cargo:rerun-if-changed=vendor/xatlas/xatlas.cpp");
    println!("cargo:rerun-if-changed=vendor/xatlas/xatlas.h");
    println!("cargo:rerun-if-changed=vendor/stb/stb_image.h");
    println!("cargo:rerun-if-changed=vendor/stb/stb_image_write.h");

    cc::Build::new()
        .cpp(true)
        .file("src/atlas_wrapper.cpp")
        .file("vendor/xatlas/xatlas.cpp")
        .include("vendor/xatlas")
        .include("vendor/stb")
        .flag_if_supported("-std=c++17")
        .warnings(false)
        .compile("meshharbor_xatlas");
}
