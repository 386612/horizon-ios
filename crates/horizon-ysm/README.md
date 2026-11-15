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
for YSM formats 4 and newer. Formats 16–25 retain category sub-entity geometry and shared player/arm model records; formats 26+ parse the vehicle/projectile category layout and later shared-name model records. The Horizon client converts the main model and extracted
base texture into its avatar renderer. Modern formats 16–25 use the sub-entity
and image-file resource layout. Numeric keyframes can drive the metadata preview animation (or the first
available animation when no preview is declared). Numeric Molang expressions
support arithmetic, comparisons, boolean and ternary expressions, common math functions,
and player-state queries such as movement, ground contact, sprinting, health, head rotation, fluid immersion and velocity. Legacy
controller state is tracked per entity; supported numeric Molang conditions
select transitions, and multiple weighted animations can be blended. The full
Bedrock/Molang language and entity-query catalog, event execution, and
`blend_transitions` map selection are not yet implemented; direct
`blend_transition` durations are applied when controller states change. Non-PNG modern image codecs (BMP/JPEG/WEBP/AVIF), complete stateful controller execution, extra-animation UI, and the full first-person texture path are not yet complete. The separate
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

### Real-file smoke test

To run the parser against a local sample without adding the model file to the
repository, set `HORIZON_YSM_TEST_FILE`:

```sh
HORIZON_YSM_TEST_FILE=/path/to/1.ysm cargo test --locked -p horizon-ysm loads_external_ysm_fixture_when_configured -- --nocapture
```

The test decrypts the container, parses the resource tables, and checks that a
main model and at least one texture were recovered.

For formats 4–25, the loader also reads the YSM metadata footer (model name,
tips, license, authors, links, scaling values, default texture, preview
animation, and extra-animation declarations). The default texture and preview
animation are used when present; `active_animation.txt` can override the
preview animation by exact animation name. This metadata support does not yet provide
the full in-game extra-animation selector or complete stateful animation-controller behavior.

## Upstream algorithm and attribution

The Crypto v3 pipeline follows the public OpenYSM/YSMParser implementation.
The modified CityHash source is vendored from OpenYSM/YSMParser's
`external/cityhash` directory and retains its upstream MIT copyright and
license notice. The upstream YSMParser repository is MIT-licensed. The modern category parser follows the official format-26 and later entity ordering, including its section marker and per-category model-name tables.
