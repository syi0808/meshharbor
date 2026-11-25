#[cfg(feature = "native")]
use std::{env, path::PathBuf, process::Command};

fn main() {
    #[cfg(feature = "native")]
    build_native();
}

#[cfg(feature = "native")]
fn build_native() {
    let source = include_path("MESHHARBOR_QUADRIFLOW_SOURCE", "src/parametrizer.hpp");
    let eigen = include_path("MESHHARBOR_EIGEN_INCLUDE", "Eigen/Core");
    let boost = include_path("MESHHARBOR_BOOST_INCLUDE", "boost/graph/adjacency_list.hpp");
    let manifest_path = "../../registry/backends/quadriflow.json";
    println!("cargo:rerun-if-changed={manifest_path}");
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../registry/backends/quadriflow.json")).unwrap();
    let commit = manifest["source"]["commit"].as_str().unwrap();
    let revision = Command::new("git")
        .current_dir(&source)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git is required to check the QuadriFlow source revision");
    assert!(
        revision.status.success() && String::from_utf8_lossy(&revision.stdout).trim() == commit,
        "MESHHARBOR_QUADRIFLOW_SOURCE must be a Git checkout of QuadriFlow {commit}"
    );
    let changes = Command::new("git")
        .current_dir(&source)
        .args([
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "src",
            "3rd",
        ])
        .output()
        .expect("could not check the QuadriFlow source tree");
    assert!(
        changes.status.success() && changes.stdout.is_empty(),
        "QuadriFlow src and 3rd must be unmodified"
    );
    for input in ["src", "3rd/lemon-1.3.1/lemon", "3rd/pcg32"] {
        println!("cargo:rerun-if-changed={}", source.join(input).display());
    }
    println!("cargo:rerun-if-changed={}", eigen.join("Eigen").display());
    println!("cargo:rerun-if-changed={}", boost.join("boost").display());
    println!("cargo:rerun-if-changed=src");

    let source_dir = source.join("src");
    let lemon = source.join("3rd/lemon-1.3.1");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++14")
        .opt_level(2)
        .define("EIGEN_MPL2_ONLY", None)
        .include("src")
        .include(&source_dir)
        .include(&lemon)
        .include(source.join("3rd/pcg32"))
        .include(eigen)
        .include(boost)
        .warnings(false)
        .file("src/quad_wrapper.cpp")
        .file(lemon.join("lemon/base.cc"));
    if build.get_compiler().is_like_msvc() {
        build
            .define("_USE_MATH_DEFINES", None)
            .define("_CRT_SECURE_NO_WARNINGS", None)
            .flag("/EHsc");
    }
    for file in [
        "adjacent-matrix.cpp",
        "dedge.cpp",
        "hierarchy.cpp",
        "loader.cpp",
        "localsat.cpp",
        "merge-vertex.cpp",
        "optimizer.cpp",
        "parametrizer.cpp",
        "parametrizer-flip.cpp",
        "parametrizer-int.cpp",
        "parametrizer-mesh.cpp",
        "parametrizer-scale.cpp",
        "parametrizer-sing.cpp",
        "subdivide.cpp",
    ] {
        build.file(source_dir.join(file));
    }
    build.compile("meshharbor_quadriflow");
}

#[cfg(feature = "native")]
fn include_path(variable: &str, required_file: &str) -> PathBuf {
    println!("cargo:rerun-if-env-changed={variable}");
    let path = env::var_os(variable)
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
        .filter(|path| path.join(required_file).is_file());
    path.unwrap_or_else(|| panic!("Set {variable} to a directory containing {required_file}"))
}
