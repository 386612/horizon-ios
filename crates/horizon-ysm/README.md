# horizon-ysm

Initial YSM container support for Horizon.

This crate currently recognizes the YSGP signature and parses the plaintext
header before the NUL-separated encrypted payload. It exposes the format and
crypto versions plus metadata fields.

**This is not yet a complete YSM loader.** It does not decrypt the payload,
decode geometry, load textures, evaluate Molang, or play animations. Those
steps are required before a model can be rendered in-game.

The parser is intentionally kept independent of Horizon's renderer so the
container format work can be tested separately and reused by a future
`.hmod` asset loader.
