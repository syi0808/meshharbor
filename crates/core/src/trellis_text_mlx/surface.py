import ast

import numpy as np


CORNERS = np.array([(i & 1, (i >> 1) & 1, i >> 2) for i in range(8)], dtype=np.int32)
EDGES = np.array([(0, 1), (1, 5), (4, 5), (0, 4), (2, 3), (3, 7), (6, 7), (2, 6),
                  (2, 0), (3, 1), (7, 5), (6, 4)], dtype=np.int32)
EDGE_AXES = np.argmax(np.abs(CORNERS[EDGES[:, 0]] - CORNERS[EDGES[:, 1]]), axis=1)


def load_tables(path):
    shapes = {"dmc_table": (256, 4, 7), "check_table": (256, 5)}
    parsed = ast.parse(path.read_text(encoding="utf-8"))
    tables = {}
    for statement in parsed.body:
        if (isinstance(statement, ast.Assign) and len(statement.targets) == 1
                and isinstance(statement.targets[0], ast.Name)):
            name = statement.targets[0].id
            if name in shapes:
                table = np.array(ast.literal_eval(statement.value), dtype=np.int32)
                if table.shape != shapes[name]:
                    raise ValueError(f"Unexpected FlexiCubes table size: {name}.")
                tables[name] = table
    if tables.keys() != shapes.keys():
        raise ValueError("Missing FlexiCubes lookup tables.")
    return tables["dmc_table"], tables["check_table"]


def grid_attributes(coords, features, resolution):
    size = resolution + 1
    shape = (size,) * 3
    counts = np.zeros(shape, dtype=np.uint8)
    attrs = np.zeros((*shape, 4), dtype=np.float32)
    deformation = features[:, 8:32].reshape(-1, 8, 3)
    for corner, offset in enumerate(CORNERS):
        indices = tuple((coords + offset).T)
        counts[indices] += 1
        attrs[indices + (0,)] += features[:, corner] - 1.0 / resolution
        attrs[indices + (slice(1, 4),)] += deformation[:, corner]
    attrs /= np.maximum(counts, 1)[..., None]
    attrs[..., 0][counts == 0] = 1
    return attrs


def surface_cases(sdf, resolution, check_table):
    occupancy = sdf < 0
    case_grid = np.zeros((resolution,) * 3, dtype=np.uint8)
    for corner, offset in enumerate(CORNERS):
        corner_slice = tuple(slice(start, start + resolution) for start in offset)
        case_grid |= occupancy[corner_slice].astype(np.uint8) << corner
    cells = np.argwhere((case_grid != 0) & (case_grid != 255)).astype(np.int32)
    case_ids = case_grid[tuple(cells.T)]
    configs = check_table[case_ids]
    candidates = np.flatnonzero(configs[:, 0] == 1)
    adjacent = cells[candidates] + configs[candidates, 1:4]
    in_bounds = np.all((adjacent >= 0) & (adjacent < resolution), axis=1)
    adjacent_cases = case_grid[tuple(adjacent[in_bounds].T)]
    invert = candidates[in_bounds][check_table[adjacent_cases, 0] == 1]
    case_ids[invert] = configs[invert, -1]
    return cells, case_ids


def cell_weights(coords, features, cells, resolution):
    indices = np.ravel_multi_index(coords.T, (resolution,) * 3)
    order = np.argsort(indices)
    ordered = indices[order]
    wanted = np.ravel_multi_index(cells.T, (resolution,) * 3)
    locations = np.searchsorted(ordered, wanted)
    locations = np.minimum(len(ordered) - 1, locations)
    found = ordered[locations] == wanted
    weights = np.zeros((len(cells), 21), dtype=np.float32)
    weights[found] = features[order[locations[found]], 32:53]
    return weights


def extract_surface(coords, features, resolution, tables):
    if not len(coords):
        raise ValueError("No occupied cells to mesh.")
    if not np.isfinite(features).all():
        raise ValueError("TRELLIS mesh decoder produced non-finite features.")
    dual_table, check_table = tables
    attrs = grid_attributes(coords, features, resolution)
    cells, case_ids = surface_cases(attrs[..., 0], resolution, check_table)
    if not len(cells):
        raise ValueError("TRELLIS mesh decoder produced no surface.")
    weights = cell_weights(coords, features, cells, resolution)
    groups = dual_table[case_ids]
    dual_mask = groups[..., 0] >= 0
    dual_number = np.cumsum(dual_mask.ravel()).reshape(dual_mask.shape) - 1
    cell_ids, slots, terms = np.nonzero(groups >= 0)
    edge_ids = groups[cell_ids, slots, terms]
    dual_ids = dual_number[cell_ids, slots]

    corners = EDGES[edge_ids]
    endpoints = cells[cell_ids, None, :] + CORNERS[corners]
    endpoint_attrs = attrs[tuple(endpoints.transpose(2, 0, 1))]
    positions = endpoints.astype(np.float32) / resolution - 0.5
    positions += np.tanh(endpoint_attrs[..., 1:]) / (2 * resolution)
    alpha = 1.0 + 0.99 * np.tanh(weights[cell_ids[:, None], 12 + corners])
    scaled_sdf = endpoint_attrs[..., 0] * alpha
    fraction = scaled_sdf[:, :1] / (scaled_sdf[:, :1] - scaled_sdf[:, 1:])
    crossings = positions[:, 0] + fraction * (positions[:, 1] - positions[:, 0])
    beta = 1.0 + 0.99 * np.tanh(weights[cell_ids, edge_ids])
    vertices = np.zeros((int(dual_mask.sum()), 3), dtype=np.float32)
    totals = np.zeros(len(vertices), dtype=np.float32)
    np.add.at(vertices, dual_ids, crossings * beta[:, None])
    np.add.at(totals, dual_ids, beta)
    vertices /= totals[:, None]

    gamma = 0.005 + 0.99 / (1.0 + np.exp(-weights[:, 20].clip(-80, 80)))
    gamma = np.broadcast_to(gamma[:, None], dual_mask.shape)[dual_mask]
    edge_keys = np.ravel_multi_index(endpoints[:, 0].T, (resolution + 1,) * 3) * 3 + EDGE_AXES[edge_ids]
    edge_order = np.argsort(edge_keys, kind="stable")
    edge_keys = edge_keys[edge_order]
    starts = np.flatnonzero(np.r_[True, edge_keys[1:] != edge_keys[:-1]])
    lengths = np.diff(np.r_[starts, len(edge_keys)])
    complete = starts[lengths == 4]
    if not len(complete):
        raise ValueError("TRELLIS mesh decoder produced no faces.")
    edge_groups = edge_order[complete[:, None] + np.arange(4)]
    quads = dual_ids[edge_groups]
    flipping = endpoint_attrs[edge_groups[:, 0], 0, 0] > 0
    quads = np.where(flipping[:, None], quads[:, [0, 1, 3, 2]], quads[:, [2, 3, 1, 0]])
    diagonals = gamma[quads]
    prefer_first = diagonals[:, 0] * diagonals[:, 2] > diagonals[:, 1] * diagonals[:, 3]
    faces = np.where(prefer_first[:, None], quads[:, [0, 1, 2, 0, 2, 3]], quads[:, [0, 1, 3, 3, 1, 2]])
    return vertices, faces.astype(np.int32).reshape(-1, 3)
