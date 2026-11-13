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
serialized YSM resource stream. The crate also has bounded geometry and RGBA-to-PNG texture record decoders, plus a legacy resource-table reader for YSM format versions 4–9. Animation records are currently indexed/skipped rather than converted into runtime keyframe tracks. Full modern-format resource parsing and connection to the game's player renderer remain future work.

## Direct `.ysm` model storage

The model directory is relative to Horizon's game working directory:

```text
config/
└── ysm/
    └── models/
        ├── model1.ysm
        └── model2.ysm
```

Horizon creates this directory and discovers `.ysm` files directly. No
`.hmod` wrapper is required.

## Upstream algorithm and attribution

The Crypto v3 pipeline follows the public OpenYSM/YSMParser implementation.
The modified CityHash source is vendored from OpenYSM/YSMParser's
`external/cityhash` directory and retains its upstream MIT copyright and
license notice. The upstream YSMParser repository is MIT-licensed.
