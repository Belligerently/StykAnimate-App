# Engine: file formats

The engine reads and writes StykAnimate's native files in
`engine/crates/styk-formats` and Pivot's files in `engine/crates/styk-piv`
([formats-piv.md](formats-piv.md)). The JavaScript reader
(`src/native-container.mjs`) is the oracle for native files; the Python codec
for Pivot files.

## Native containers (`.styk`, `.stkz`, `.stykmov`)

- `container::decode(bytes, kind)` reads version 0 (plain JSON), version 1
  (zlib) and version 2 (a zstd-compressed skeleton plus binary sections:
  number arrays and payloads) into a `styk_doc::Value`. `decode_parts` returns
  the skeleton and sections without lifting them back.
- `container::lift` / `lift_within` split a document into skeleton and
  sections the way `liftDocument` does (the mirror and saves use the same
  layout); `parse_lifted` reads one lifted record (the mirror's unit).
- `container::encode_v2` writes version 2 (zstd level 3, [D5](decisions/0005-zstd-from-c.md));
  `encode_v1` writes version 1.
- `native::normalize_decoded`, `validate_figure` and `verify_embedded_source`
  apply the native document rules after decoding, with the editor's error keys
  (`ContainerError::message_key`, `NativeError::message_key`).

Parity: all 461 corpus files decode to the JavaScript reader's document; all
448 version 2 files rewrite byte-identical. Repository files are checked by
`npm run engine:parity`.

## Opening a file in the engine

`styk_eval::open` / `open_bytes` pick by extension: native containers through
`styk-formats`, `.piv` and `.stk` through `styk_piv::decode_document`, so the
engine reads exactly the document the editor imports. Both produce a tree that
`styk_eval::from_value` turns into typed views.

## Changing a format

Add the field where the JavaScript writer writes it, keep unknown fields
flowing through the tree (no typed view is needed to preserve a field), add a
corpus or fixture file that has it, and check `npm run engine:parity`. A new
container version needs a reader here and in `src/native-container.mjs`
until the JavaScript reader is removed.
