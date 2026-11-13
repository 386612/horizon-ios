# horizon-ysm

Initial YSM container support for Horizon.

This crate currently:
- recognizes the YSGP signature (with or without a UTF-8 BOM);
- parses the plaintext header and metadata fields;
- reads the Crypto Version 3 marker;
- extracts the encrypted payload and the trailing 32-byte key, 24-byte IV
  and 8-byte stored hash;
- rejects truncated containers and inconsistent/unsupported crypto versions.

It does **not** yet verify the stored CityHash, decrypt the payload, reverse
YSM's Zstandard block transformation, decode model geometry, load textures,
evaluate Molang, play animations, or integrate with the game's player renderer.
It is not yet a runtime model loader.

## Direct `.ysm` model storage

The planned location for user-imported YSM files is:

```text
config/
└── ysm/
    └── models/
        ├── model1.ysm
        └── model2.ysm
```

The path is relative to Horizon's game working directory (on iOS, the app's
game Documents directory). Users can place multiple `.ysm` files here without
wrapping them in `.hmod`. The `.hmod` content-pack format is not required for
this direct-import workflow.

This is the agreed storage convention; the current build does not yet create,
scan, or load this directory. A later integration step must connect the
directory to the YSM decryptor, model/texture/animation decoders, and native
player rendering hook.
