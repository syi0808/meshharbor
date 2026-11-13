mod runtime_bundle {
    include!(concat!(env!("OUT_DIR"), "/pixal3d_runtime_bundle.rs"));
}

mod generation;
mod cuda_runtime;
mod glb;
mod operation;
mod runtime;
mod uv_bootstrap;
mod store;

pub use cuda_runtime::Pixal3dCudaRuntimeManager;
pub use generation::{GenerationOptions, GenerationResult, Pixal3dMlxRuntime, RuntimeReadiness};
pub use glb::{AssetDescriptor, SimplificationReport, inspect_asset, simplify_glb};
pub use operation::{OperationCore, OperationDescriptor, RuntimeDoctor, SimplificationResult};
pub use runtime::{Pixal3dRuntimeManager, RuntimeSetupReport};
pub use store::{Artifact, Asset, AssetKind, CoreError, CoreResult, Job, JobState, Store};
