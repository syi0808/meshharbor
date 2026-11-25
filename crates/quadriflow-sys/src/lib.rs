#![cfg(feature = "native")]

unsafe extern "C" {
    /// Runs the solver once in a disposable worker process and writes `output.obj` in its cwd.
    ///
    /// # Safety
    /// Both pointers must cover the given numbers of packed triples for the whole call.
    /// The input must be validated closed, oriented, manifold triangle geometry with
    /// finite positions, in-range indices, and counts and target within the solver limits.
    pub fn meshharbor_quadriflow_run(
        positions: *const f32,
        vertex_count: usize,
        indices: *const u32,
        triangle_count: usize,
        target_quads: u32,
    ) -> std::ffi::c_int;
}
