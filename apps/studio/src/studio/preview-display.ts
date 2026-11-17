export interface PreviewMaterialTarget {
  material: unknown;
}

export function applyTriangleWireframe<
  Mesh extends PreviewMaterialTarget,
  Material,
>(
  meshes: readonly Mesh[],
  wireframeMaterial: Material,
  originalMaterials: Map<Mesh, Mesh['material']>,
) {
  for (const mesh of meshes) {
    if (originalMaterials.has(mesh)) continue;

    const original = mesh.material;
    originalMaterials.set(mesh, original);
    mesh.material = (
      Array.isArray(original)
        ? original.map(() => wireframeMaterial)
        : wireframeMaterial
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


// Model JSON rejects explicit external buffer/image URLs before this check.
// Three.js exposes image bufferViews through same-origin browser object URLs.
export function isLocalPreviewResource(url: string, origin: string): boolean {
  return url.startsWith('data:') || url.startsWith(`blob:${origin}/`);
}
