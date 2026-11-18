const rings = 20;
const sides = 8;

function buildStudy() {
  const points = Array.from({ length: rings * sides }, (_, index) => {
    const around = Math.floor(index / sides) * 2 * Math.PI / rings;
    const cross = (index % sides) * 2 * Math.PI / sides;
    const radius = 1.65 + 0.55 * Math.cos(cross);
    const horizontal = radius * Math.cos(around);
    const vertical = radius * Math.sin(around);
    const elevation = 0.55 * Math.sin(cross);
    return {
      screenX: 200 + 74 * (horizontal * 0.94 - vertical * 0.34),
      screenY: 140 + 74 * (vertical * 0.5 + horizontal * 0.18 - elevation * 0.84),
      depth: vertical * 0.79 + horizontal * 0.29 + elevation * 0.53,
    };
  });
  const vertex = (ring: number, side: number) => points[(ring % rings) * sides + side % sides];
  return points.flatMap((_, index) => {
    const ring = Math.floor(index / sides);
    const side = index % sides;
    const first = vertex(ring, side);
    const nextRing = vertex(ring + 1, side);
    const nextSide = vertex(ring, side + 1);
    const opposite = vertex(ring + 1, side + 1);
    return [[first, nextSide, opposite], [first, opposite, nextRing]].flatMap(([start, middle, end]) => {
      const winding = (middle.screenX - start.screenX) * (end.screenY - start.screenY)
        - (middle.screenY - start.screenY) * (end.screenX - start.screenX);
      if (winding < 0) return [];
      return [{
        key: `${index}-${middle === nextSide ? 'side' : 'ring'}`,
        points: [start, middle, end].map(point => `${point.screenX.toFixed(1)},${point.screenY.toFixed(1)}`).join(' '),
        depth: (start.depth + middle.depth + end.depth) / 3,
        shade: Math.min(0.38, 0.10 + winding / 1600),
      }];
    });
  }).sort((left, right) => left.depth - right.depth);
}

const study = buildStudy();

export function MeshStudy() {
  return <svg className="mesh-study" viewBox="0 0 400 280" aria-hidden="true" focusable="false">
    <g strokeWidth=".7" strokeLinejoin="round">
      {study.map(face => <polygon key={face.key} points={face.points}
        fill={`color-mix(in srgb, var(--studio-accent) ${face.shade * 100}%, var(--studio-canvas))`}
        stroke="var(--studio-study-edge)" />)}
    </g>
  </svg>;
}
