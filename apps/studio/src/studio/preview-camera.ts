export type PreviewVec3 = readonly [number, number, number];

export interface PreviewBounds {
  min: PreviewVec3;
  max: PreviewVec3;
}

export interface PreviewFrame {
  /** The source-space point translated to the preview origin. */
  origin: PreviewVec3;
  /** The framed center after applying the origin translation. */
  target: PreviewVec3;
  size: PreviewVec3;
  maxDimension: number;
}

/**
 * Build a stable frame from world-space bounds. Comparison previews use the
 * original asset's frame for both sides, rather than each side's own center.
 */
export function previewFrameFromBounds(
  bounds: PreviewBounds,
  origin: PreviewVec3 = midpoint(bounds.min, bounds.max),
): PreviewFrame | null {
  const values = [...bounds.min, ...bounds.max, ...origin];
  if (values.some((value) => !Number.isFinite(value))) return null;

  const size: PreviewVec3 = [
    bounds.max[0] - bounds.min[0],
    bounds.max[1] - bounds.min[1],
    bounds.max[2] - bounds.min[2],
  ];
  if (size.some((value) => value < 0)) return null;

  const center = midpoint(bounds.min, bounds.max);
  const target: PreviewVec3 = [
    center[0] - origin[0],
    center[1] - origin[1],
    center[2] - origin[2],
  ];
  return {
    origin,
    target,
    size,
    maxDimension: Math.max(size[0], size[1], size[2], 0.001),
  };
}

/** Camera state belongs to a pair, and survives only a side switch in that pair. */
export function shouldPreserveComparisonCamera(
  previousGroupId: string | null,
  nextGroupId: string | null,
): boolean {
  return previousGroupId !== null && nextGroupId !== null && previousGroupId === nextGroupId;
}

/**
 * A pair may only become known after a job finishes. Keep the frame if the
 * currently displayed asset is exactly the newly identified source asset.
 */
export function shouldPreserveSourceTransition(
  previousAssetId: string | null,
  sourceAssetId: string,
): boolean {
  return previousAssetId !== null && previousAssetId === sourceAssetId;
}

function midpoint(min: PreviewVec3, max: PreviewVec3): PreviewVec3 {
  return [
    min[0] + (max[0] - min[0]) / 2,
    min[1] + (max[1] - min[1]) / 2,
    min[2] + (max[2] - min[2]) / 2,
  ];
}
