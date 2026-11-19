import json as _meshharbor_json
import os as _meshharbor_os


_meshharbor_sampler_overrides = _meshharbor_json.loads(
    _meshharbor_os.environ.pop("MESHHARBOR_PIXAL3D_SAMPLERS", "{}")
)


def _meshharbor_tune_sampler(stage, defaults):
    params = dict(defaults)
    interval = list(params.get("guidance_interval", (0.6, 1.0)))
    for key, value in _meshharbor_sampler_overrides.get(stage, {}).items():
        if key == "guidance_start":
            interval[0] = value
        elif key == "guidance_end":
            interval[1] = value
        else:
            params[key] = value
    params["guidance_interval"] = tuple(interval)
    return params
