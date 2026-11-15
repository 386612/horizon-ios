# horizon-ysm

YSM container and Crypto v3 support for Horizon.

This crate currently:
- recognizes the YSGP signature (with or without a UTF-8 BOM);
- parses plaintext header metadata and validates Crypto Version 3;
- extracts the encrypted payload, key, IV and stored file hash;
- verifies the file hash using YSM's modified CityHash64 constants;
- decrypts the payload using YSM's variable-round modified XChaCha20 stage;
- reverses the MT19937-64 XOR stage with YSM's CityHash-derived seed;
- removes the per-file nonce prefix;
- restores YSM's modified Zstandard frame/block headers and decompresses the resource stream.

The public entry point is `decrypt_container(file_bytes)`. It returns the decompressed
serialized YSM resource stream. The crate also has bounded geometry, texture, animation, animation-controller, sound and metadata decoders
for YSM formats 4–15. The Horizon client converts the main model and extracted
base texture into its avatar renderer. Numeric keyframes can drive the metadata preview animation (or the first
available animation when no preview is declared). Numeric Molang expressions
support arithmetic, comparisons, boolean and ternary expressions, common math functions,
and player-state queries such as movement, ground contact, sprinting, health, head rotation, fluid immersion and velocity. Legacy
controller initial-state animations and transitions with supported numeric
Molang conditions can select tracks. The full Bedrock/Molang language and entity-query catalog, event execution,
and blend-transition behavior are not yet implemented. Formats above 15 (the modern sub-entity resource layout), complete stateful controller execution, extra-animation UI, and the full first-person texture path are not yet complete. The separate
YSM arm geometry is loaded when available.

## Direct `.ysm` model storage

The model directory is relative to Horizon's game working directory:

```text
config/
└── ysm/
    ├── active.txt        # optional exact .ysm filename to select
    ├── active_animation.txt # optional exact animation track name
    └── models/
        ├── model1.ysm
        └── model2.ysm
```

Horizon creates this directory and discovers `.ysm` files directly. Without
`active.txt`, it selects the first valid file in filename order. No `.hmod`
wrapper is required.

For formats 4–15, the loader also reads the YSM metadata footer (model name,
tips, license, authors, links, scaling values, default texture, preview
animation, and extra-animation declarations). The default texture and preview
animation are used when present; `active_animation.txt` can override the
preview animation by exact animation name. This metadata support does not yet provide
the full in-game extra-animation selector or complete stateful animation-controller behavior.

## Upstream algorithm and attribution

The Crypto v3 pipeline follows the public OpenYSM/YSMParser implementation.
The modified CityHash source is vendored from OpenYSM/YSMParser's
`external/cityhash` directory and retains its upstream MIT copyright and
license notice. The upstream YSMParser repository is MIT-licensed.
