def _cleanup_and_simplify_mesh(vertices, faces, *, target_faces, no_cleanup,
                               keep_largest=False, hole_fill_backend="fan"):
    from generate import _cleanup_and_simplify_mesh as simplify_mesh

    if not no_cleanup:
        from pixalmlx import mesh_cleanup

        vertices, faces = mesh_cleanup.remove_duplicate_faces(vertices, faces, verbose=False)
        filter_components = (
            mesh_cleanup.keep_largest_component if keep_largest
            else mesh_cleanup.remove_small_components
        )
        vertices, faces = filter_components(vertices, faces)

    vertices, faces = simplify_mesh(vertices, faces, target_faces=target_faces, no_cleanup=True)
    if no_cleanup:
        return vertices, faces

    vertices, faces = mesh_cleanup.cleanup_mesh(
        vertices, faces, min_component_area=0, do_fix_normals=False,
        hole_fill_backend=hole_fill_backend,
    )
    return mesh_cleanup.orient_faces_by_adjacency(vertices, faces, verbose=False)
