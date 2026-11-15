export interface PreviewMaterialTarget<Material> {
  material: Material | Material[];
}

export function applyTriangleWireframe<
  Material,
  Mesh extends PreviewMaterialTarget<Material>,
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
  Material,
  Mesh extends PreviewMaterialTarget<Material>,
>(originalMaterials: Map<Mesh, Mesh['material']>) {
  for (const [mesh, material] of originalMaterials) {
    mesh.material = material;
  }
  originalMaterials.clear();
}
