# Studio workspace references

Reviewed the free, signed-out public web workspaces on 2026-11-18 at 1440, 1024, 768, and 390 CSS pixels (900px tall). This is a snapshot of browser layout, not a claim about signed-in, subscription, or mobile-app behavior.

| Width | Meshy public workspace | Tripo public 3D Workspace |
| --- | --- | --- |
| 1440 | Creation settings on the left, a central viewport, searchable history/assets on the right. | Feature rail and feature form on the left, central viewport, Assets/Property on the right. |
| 1024 | Three areas remain; the history search visibly narrows. | The document retains a 1280px minimum width and the right Assets tab is clipped at the edge. |
| 768 | The creation form remains accessible; the right history search is outside the initial viewport. | The desktop-width workspace is still present, rather than an inventory drawer. |
| 390 | The top navigation condenses; the right history remains outside the initial viewport. | The 1280px workspace still overflows. This is not a model for MeshHarbor's mobile layout. |

## Feature patterns

- **Generate.** [Meshy Image to 3D](https://docs.meshy.ai/en/webapp/image-to-3d) takes an explicit source image and generation recipe before the viewer displays an output. [Tripo's public Model workspace](https://studio.tripo3d.ai/workspace/generate) uses its own upload area and model options; the right library is not an implicit input. Both also expose entry points for generation outside the viewer.
- **Edit an existing model.** [Meshy's interface tour](https://help.meshy.ai/en/articles/12618267-meshy-interface-tour-workspace-tabs-and-navigation) places Remesh, UV, and texture actions in the active model's editing toolbar. [Tripo's Retopo](https://studio.tripo3d.ai/workspace/retopology), Segment, Smart UV, and Texture views keep the same inventory and swap the tool form. Retopo, Segment, and Texture explicitly ask for a model from Assets or a new upload.
- **Review and export.** [Tripo's Studio guide](https://www.tripo3d.ai/blog/tripo-studio-tutorial-english) separates the right Assets/Property/history surfaces from tool settings. [Meshy's tour](https://help.meshy.ai/en/articles/12618267-meshy-interface-tour-workspace-tabs-and-navigation) separates generation history from the active model and its actions. MeshHarbor retains its non-destructive Original/Result and export flow instead of copying cloud/account controls.

## MeshHarbor decisions

- Above 1024px: left tool rail and resizable settings, persistent central viewport, resizable right inventory with Assets and Jobs.
- 761–1024px: keep the right inventory dock and open tool settings from a left drawer. At 760px and below, the viewport spans the window above a compact command bar; tools and inventory open from opposite sides.
- New model owns an explicit image reference. Choosing or replacing it does not select it in the viewport. From a displayed image, **Use as reference** intentionally attaches it. A new output joins the inventory without replacing a different model the user is reviewing; **View generated model** opens it when ready.
- Optimize and More tools use the selected GLB, keep the viewer mounted, and display the target filename in settings. They do not imply parity with competitors' text-to-3D, rigging, or generative texturing.
