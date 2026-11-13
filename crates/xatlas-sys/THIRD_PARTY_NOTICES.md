# Third-party notices

This crate vendors only the source needed for atlas parameterization and PNG decoding/encoding. The upstream license notices are retained in the corresponding source files.

## xatlas

- Project: [jpcy/xatlas](https://github.com/jpcy/xatlas)
- Pinned source revision: `f700c7790aaa030e794b52ba7791a05c085faf0c`
- License: MIT, copyright 2018–2020 Jonathan Young. `vendor/xatlas/LICENSE` and the license/attribution blocks in `xatlas.cpp` are retained. Those embedded blocks include the upstream Thekla/NVIDIA attribution and related notice text.

## stb_image and stb_image_write

- Project: [nothings/stb](https://github.com/nothings/stb)
- Pinned source revision: `4e9b368bfc071e97206ea5c6adba68dd0cbf8897`
- Files: `vendor/stb/stb_image.h` and `vendor/stb/stb_image_write.h`
- License: public domain or MIT dual license. The full alternatives and attribution are preserved inside each vendored header; this project uses the MIT option.

No source from the experimental `glb-mesh-refiner` executable is copied. The atlas wrapper and texture resampler in `src/atlas_wrapper.cpp` are new MeshHarbor integration code.
