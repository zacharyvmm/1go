# HTML entity table generation

`entities.json` is a committed copy of the WHATWG named character
reference dataset.

The fixture and generator scripts under this directory are intentionally
excluded from the published `scah` crate. Runtime code uses only the
generated Rust table in `src/html/entities_table.rs`.

## Regenerate from the committed fixture

```bash
python crates/scah/scripts/generate_entities.py
```

This command is offline and deterministic. It never contacts the network
and never inserts a wall-clock timestamp into generated output.

## Update from WHATWG

```bash
python crates/scah/scripts/generate_entities.py --update-fixture
```

This downloads the upstream dataset and updates both the committed
fixture and generated Rust table from the same downloaded bytes.

After updating:

```bash
python crates/scah/scripts/generate_entities.py
git diff --check
cargo test -p scah
```

## Provenance

* Source: `https://html.spec.whatwg.org/entities.json`
* Committed source SHA-256:
  `d741d877ac77c4194c4ad526b5b4a19aef8dfe411ab840a466891cdbb9f362e6`
* Third-party license:
  `THIRD_PARTY_LICENSES/WHATWG-HTML.txt`
  (mirrored for packaging at
  `crates/scah/THIRD_PARTY_LICENSES/WHATWG-HTML.txt`)

## How to update the fixture

1. Run `--update-fixture` (see above).
2. Confirm the printed SHA-256 and entry count.
3. Regenerate once more without network access and confirm a clean diff.
4. Update the SHA-256 recorded in this README and in the third-party
   license notice if the upstream bytes changed.
5. Commit the fixture, generated table, and notice updates together.
