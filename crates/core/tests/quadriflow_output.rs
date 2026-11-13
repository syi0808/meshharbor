//! End-to-end validation of a real QuadriFlow output, enabled only by the native build smoke workflow.

use meshharbor_core::{
    AssetKind, inspect_asset, parse_quad_obj_output, quad_mesh_preview_glb,
};

#[test]
#[ignore = "requires the separately built pinned QuadriFlow smoke executable"]
fn validates_quad_output_and_builds_a_glb_preview() {
    let path = std::env::var_os("MESHHARBOR_QUADRIFLOW_OBJ_OUTPUT")
        .expect("the QuadriFlow smoke workflow must set its output path");
    let bytes = std::fs::read(path).expect("the native smoke run must produce its OBJ output");
    let source_positions = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let (mesh, report) = parse_quad_obj_output(&bytes, &source_positions)
        .expect("QuadriFlow should produce a closed, valid quad mesh for the cube");
    assert!(report.output_quads > 0);
    assert_eq!(report.preview_triangles, report.output_quads * 2);

    let preview = quad_mesh_preview_glb(&mesh).expect("quad preview GLB should be valid");
    let descriptor = inspect_asset("quadriflow-cube-preview.glb", &preview)
        .expect("the generated preview should pass the GLB inspector");
    assert_eq!(descriptor.kind, AssetKind::Mesh);
    assert_eq!(
        descriptor.metadata["triangles"],
        report.preview_triangles
    );
}
