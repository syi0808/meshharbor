use std::{collections::BTreeMap, process::Command, sync::LazyLock};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{CoreError, CoreResult, GenerationOptions};

#[derive(Deserialize)]
struct Parameter {
    id: String,
    kind: String,
    #[serde(default)]
    values: Vec<Value>,
    min: Option<f64>,
    max: Option<f64>,
    #[serde(default)]
    single_view_only: bool,
    sampler: Option<[String; 2]>,
}

struct Catalog {
    definitions: Vec<Value>,
    parameters: Vec<Parameter>,
}

static TUNING: LazyLock<BTreeMap<String, Catalog>> = LazyLock::new(|| {
    let mut payload: BTreeMap<String, Vec<Value>> =
        serde_json::from_str(include_str!("../../../registry/models/pixal3d-tuning.json"))
            .expect("bundled Pixal3D tuning catalog is valid");
    let cuda = payload
        .get_mut("pixal3d-cuda")
        .expect("bundled CUDA catalog");
    let mut sampling = Vec::new();
    let mut mlx_sampling = Vec::new();
    for (id, label, key, strength, rescale, end, time_rescale) in [
        (
            "structure",
            "Structure",
            "sparse_structure_sampler",
            7.5,
            0.7,
            1.0,
            5.0,
        ),
        (
            "shape",
            "Shape (both passes)",
            "shape_slat_sampler",
            7.5,
            0.5,
            1.0,
            3.0,
        ),
        ("texture", "Texture", "tex_slat_sampler", 1.0, 0.0, 0.9, 3.0),
    ] {
        for (parameter, title, kind, default, min, max) in [
            ("steps", "sampling steps", "integer", json!(12), 1.0, 100.0),
            (
                "guidance_strength",
                "guidance strength",
                "number",
                json!(strength),
                0.0,
                30.0,
            ),
            (
                "guidance_rescale",
                "guidance rescale",
                "number",
                json!(rescale),
                0.0,
                1.0,
            ),
            (
                "guidance_start",
                "guidance start",
                "number",
                json!(0.6),
                0.0,
                1.0,
            ),
            (
                "guidance_end",
                "guidance end",
                "number",
                json!(end),
                0.0,
                1.0,
            ),
            (
                "rescale_t",
                "timestep rescale",
                "number",
                json!(time_rescale),
                0.01,
                20.0,
            ),
        ] {
            let definition = json!({
                "id": format!("{id}_{parameter}"),
                "label": format!("{label} {title}"),
                "section": "Sampling",
                "kind": kind,
                "default": default,
                "min": min,
                "max": max,
                "sampler": [key, parameter],
            });
            if parameter != "steps" {
                mlx_sampling.push(definition.clone());
            }
            sampling.push(definition);
        }
    }
    cuda.splice(0..0, sampling);
    payload
        .get_mut("pixal3d-mlx")
        .expect("bundled MLX catalog")
        .splice(4..4, mlx_sampling);
    payload
        .into_iter()
        .map(|(id, definitions)| {
            let parameters = definitions
                .iter()
                .cloned()
                .map(|definition| {
                    serde_json::from_value(definition).expect("bundled tuning definition")
                })
                .collect();
            (
                id,
                Catalog {
                    definitions,
                    parameters,
                },
            )
        })
        .collect()
});

pub(crate) fn definitions(backend_id: &str) -> impl Iterator<Item = Value> + '_ {
    TUNING[backend_id]
        .definitions
        .iter()
        .cloned()
        .map(|mut definition| {
            let object = definition.as_object_mut().expect("catalog parameter");
            object.remove("sampler");
            object.insert("tuning".into(), Value::Bool(true));
            definition
        })
}

fn default_number(backend_id: &str, id: &str) -> f64 {
    TUNING[backend_id]
        .definitions
        .iter()
        .find(|entry| entry["id"] == id)
        .and_then(|entry| entry["default"].as_f64())
        .expect("bundled parameter default")
}

pub(crate) fn validate(options: &GenerationOptions, backend_id: &str) -> CoreResult<()> {
    let catalog = &TUNING[backend_id];
    for (id, value) in &options.tuning {
        let parameter = catalog
            .parameters
            .iter()
            .find(|parameter| &parameter.id == id)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "Unknown generation setting '{id}' for {backend_id}."
                ))
            })?;
        if options.view_input.is_some() && parameter.single_view_only {
            return Err(CoreError::InvalidInput(format!(
                "{id} is supplied by each view’s camera."
            )));
        }
        let in_range = |number: f64| {
            number.is_finite()
                && parameter.min.is_none_or(|min| number >= min)
                && parameter.max.is_none_or(|max| number <= max)
        };
        let valid = match parameter.kind.as_str() {
            "boolean" => value.is_boolean(),
            "choice" => parameter.values.contains(value),
            "integer" => value.as_u64().is_some_and(|number| in_range(number as f64)),
            "number" => value.as_f64().is_some_and(in_range),
            _ => false,
        };
        if !valid || id == "max_faces" && matches!(value.as_u64(), Some(1..=3)) {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported value for generation setting '{id}'."
            )));
        }
    }
    let value = |id: &str| {
        options
            .tuning
            .get(id)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| default_number(backend_id, id))
    };
    for stage in ["structure", "shape", "texture"] {
        if value(&format!("{stage}_guidance_start")) > value(&format!("{stage}_guidance_end")) {
            return Err(CoreError::InvalidInput(format!(
                "{stage} guidance start must not exceed guidance end."
            )));
        }
    }
    Ok(())
}

pub(crate) fn has_sampler_overrides(options: &GenerationOptions, backend_id: &str) -> bool {
    TUNING[backend_id]
        .parameters
        .iter()
        .any(|parameter| parameter.sampler.is_some() && options.tuning.contains_key(&parameter.id))
}

pub(crate) fn add_arguments(command: &mut Command, options: &GenerationOptions, backend_id: &str) {
    let mut samplers: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for parameter in &TUNING[backend_id].parameters {
        let Some(value) = options.tuning.get(&parameter.id) else {
            continue;
        };
        if let Some([stage, key]) = &parameter.sampler {
            samplers
                .entry(stage.clone())
                .or_default()
                .insert(key.clone(), value.clone());
        } else if parameter.id == "fov_degrees" && backend_id == "pixal3d-mlx" {
            command.arg("--fov").arg(
                value
                    .as_f64()
                    .expect("validated FOV")
                    .to_radians()
                    .to_string(),
            );
        } else {
            let flag = parameter.id.replace('_', "-");
            match value.as_bool() {
                Some(true) => {
                    command.arg(format!("--{flag}"));
                }
                Some(false) if parameter.id == "compile_transformer" => {
                    command.arg("--no-compile-transformer");
                }
                Some(false) => {}
                None => {
                    let text = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned);
                    command.arg(format!("--{flag}")).arg(text);
                }
            }
        }
    }
    if !samplers.is_empty() {
        command.env(
            "MESHHARBOR_PIXAL3D_SAMPLERS",
            serde_json::to_string(&samplers).expect("validated sampler overrides"),
        );
    }
}
