//! Optional child-process entry point for experimental triangle remeshing.
//!
//! The host and worker share a small versioned JSON protocol. This executable is
//! deliberately separate from the REST/MCP process because remesh 0.0.5 is
//! experimental, may panic, and writes progress to stdout.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use clap::Parser;
use meshharbor_core::{
    RemeshRequest as Request, RemeshResponse as Response, REMESH_PROTOCOL_VERSION,
    normalize_remesh_positions,
    restore_remesh_positions, validate_remesh_response,
};
use remesh::prelude::*;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(about = "MeshHarbor experimental triangle-remeshing worker")]
struct Args {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("meshharbor-retopo-worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    let request_size = fs::metadata(&args.input)
        .map_err(|error| format!("cannot stat request file: {error}"))?
        .len();
    if request_size > MAX_REQUEST_BYTES {
        return Err("request file exceeds the 64 MiB limit".into());
    }
    let request_bytes =
        fs::read(&args.input).map_err(|error| format!("cannot read request file: {error}"))?;
    let request: Request = serde_json::from_slice(&request_bytes)
        .map_err(|error| format!("invalid request JSON: {error}"))?;

    let response = remesh(request)?;
    let output_bytes = serde_json::to_vec(&response)
        .map_err(|error| format!("cannot encode response: {error}"))?;
    if output_bytes.len() > MAX_RESPONSE_BYTES {
        return Err("response exceeds the 64 MiB limit".into());
    }

    // Never overwrite a caller-selected existing file. The host supplies a fresh
    // response path inside its private temporary directory.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args.output)
        .map_err(|error| format!("cannot create response file: {error}"))?;
    output
        .write_all(&output_bytes)
        .map_err(|error| format!("cannot write response file: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("cannot flush response file: {error}"))?;
    Ok(())
}

fn remesh(request: Request) -> Result<Response, String> {
    let (normalized, center, scale_factor) =
        normalize_remesh_positions(&request).map_err(|error| error.to_string())?;
    // remesh uses u32 indices for this API version. Keep the GLB index width
    // throughout instead of round-tripping through usize.
    let remesher = IsotropicRemesh::<f32, [f32; 3]>::new(&normalized, &request.indices)
        .map_err(|error| format!("remesh input rejected: {error}"))?
        .with_target_edge_length(1.0)
        .map_err(|error| format!("invalid remesh target edge length: {error}"))?
        .with_split_edges(SplitStrategy::DihedralAngle)
        .map_err(|error| format!("cannot configure edge splitting: {error}"))?
        .with_collapse_edges(CollapseStrategy::DihedralAngle)
        .map_err(|error| format!("cannot configure edge collapsing: {error}"))?
        .with_flip_edges(FlipStrategy::Valence)
        .map_err(|error| format!("cannot configure edge flipping: {error}"))?
        .with_default_smooth_weight()
        .map_err(|error| format!("cannot configure smoothing: {error}"))?;

    let (output_positions, output_indices): (Vec<[f32; 3]>, Vec<u32>) = remesher
        .run(request.options.iterations as usize)
        .map_err(|error| format!("isotropic remeshing failed: {error}"))?;

    let positions = restore_remesh_positions(&output_positions, center, scale_factor)
        .map_err(|error| error.to_string())?;
    let response = Response {
        positions,
        indices: output_indices,
    };
    validate_remesh_response(&response, &request).map_err(|error| error.to_string())?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshharbor_core::{RetopologyOptions as Options, validate_remesh_request};

    #[test]
    fn rejects_open_untextured_triangle_patch() {
        let request = Request {
            protocol_version: REMESH_PROTOCOL_VERSION,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            options: Options {
                target_edge_ratio: 0.2,
                iterations: 1,
            },
        };
        assert!(
            validate_remesh_request(&request)
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
    }

    #[test]
    fn remeshes_closed_untextured_tetrahedron() {
        let request = Request {
            protocol_version: REMESH_PROTOCOL_VERSION,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
            options: Options {
                target_edge_ratio: 0.2,
                iterations: 1,
            },
        };
        let output = remesh(request).expect("closed tetrahedron should remesh");
        assert!(!output.positions.is_empty());
        assert!(!output.indices.is_empty());
        assert_eq!(output.indices.len() % 3, 0);
        assert!(
            output
                .indices
                .iter()
                .all(|index| (*index as usize) < output.positions.len())
        );
    }
}
