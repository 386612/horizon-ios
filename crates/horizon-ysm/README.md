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
evaluate Molang, or play animations. It is not yet a runtime model loader.

## Intended content-pack layout

Horizon content mods use namespaced resource paths. The intended layout for a
future YSM-aware Horizon build is:

```text
Aru.hmod
├── horizon-mod.toml
└── assets/
    └── aru/
        └── ysm/
            └── 1.ysm
```

The `.ysm` path is a proposed convention, not a path the current game loads
as a player model. A later integration step must connect this crate to the
content/resource loader and the native player rendering hook.
