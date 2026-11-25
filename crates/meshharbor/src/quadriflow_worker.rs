use std::{ffi::OsString, io::Read};

use meshharbor_core::{
    MAX_LINKED_QUADRIFLOW_REQUEST_BYTES, MAX_QUADRIFLOW_TARGET, RemeshRequest,
    validate_remesh_request,
};

pub(crate) fn run(
    mut args: impl Iterator<Item = OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_quads = args
        .next()
        .and_then(|value| value.to_str()?.parse::<u32>().ok())
        .filter(|target| (4..=MAX_QUADRIFLOW_TARGET).contains(target))
        .ok_or("Invalid QuadriFlow worker target.")?;
    if args.next().is_some() {
        return Err("Unexpected QuadriFlow worker arguments.".into());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_LINKED_QUADRIFLOW_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LINKED_QUADRIFLOW_REQUEST_BYTES {
        return Err("QuadriFlow worker request exceeds 64 MiB.".into());
    }
    let request: RemeshRequest = serde_json::from_slice(&bytes)?;
    drop(bytes);
    validate_remesh_request(&request)?;
    let status = unsafe {
        meshharbor_quadriflow_sys::meshharbor_quadriflow_run(
            request.positions.as_ptr().cast(),
            request.positions.len(),
            request.indices.as_ptr(),
            request.indices.len() / 3,
            target_quads,
        )
    };
    if status != 0 {
        return Err("QuadriFlow native solver failed.".into());
    }
    Ok(())
}
