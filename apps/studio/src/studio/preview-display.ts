export interface PreviewMaterialTarget {
  material: unknown;
}

export function applyPreviewMaterial<
  Mesh extends PreviewMaterialTarget,
  Material,
>(
  meshes: readonly Mesh[],
  previewMaterial: Material,
  originalMaterials: Map<Mesh, Mesh['material']>,
) {
  for (const mesh of meshes) {
    if (originalMaterials.has(mesh)) continue;

    const original = mesh.material;
    originalMaterials.set(mesh, original);
    mesh.material = (
      Array.isArray(original)
        ? original.map(() => previewMaterial)
        : previewMaterial
    ) as Mesh['material'];
  }
}

export function restorePreviewMaterials<
  Mesh extends PreviewMaterialTarget,
>(originalMaterials: Map<Mesh, Mesh['material']>) {
  for (const [mesh, material] of originalMaterials) {
    mesh.material = material;
  }
  originalMaterials.clear();
}

export function separatedPartInfo(metadata: Record<string, unknown>) {
  const report = metadata.report;
  if (metadata.operation !== 'mesh.separate_parts' || !report || typeof report !== 'object') return null;
  const { output_primitives: count, grouped_components: grouped } = report as Record<string, unknown>;
  if (typeof count !== 'number' || !Number.isInteger(count) || count < 1 || count > 256) return null;
  return { count, grouped: typeof grouped === 'number' && grouped > 0 };
}

export function applyPreviewPartMaterials<Mesh extends PreviewMaterialTarget, Material>(
  meshParts: ReadonlyMap<Mesh, number>,
  palette: readonly Material[],
  dimmed: Material,
  focused: number | null,
  originalMaterials: Map<Mesh, Mesh['material']>,
) {
  for (const [mesh, index] of meshParts) {
    const material = palette[index];
    if (material !== undefined) applyPreviewMaterial([mesh], focused === null || focused === index ? material : dimmed, originalMaterials);
  }
}


// Model JSON rejects explicit external buffer/image URLs before this check.
// Three.js exposes image bufferViews through same-origin browser object URLs.
export function isLocalPreviewResource(url: string, origin: string): boolean {
  return url.startsWith('data:') || url.startsWith(`blob:${origin}/`);
}
