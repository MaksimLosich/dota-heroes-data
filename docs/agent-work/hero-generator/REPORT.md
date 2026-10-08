# heroes_data implementation report

Task ID: /root/hero_generator
Workspace: `D:\StudioProjects\dota_who_revival\heroes_data`
Status: accepted

## What was built

A single-binary Rust tool that fetches Dota 2 hero data from Valve's official
datafeed, normalizes it for DotaWho, merges `overrides.json`, validates, checks hero
images, and writes `heroes.json`.

Changed paths:

- `Cargo.toml`, `Cargo.lock` - deps: anyhow, reqwest (rustls), serde, serde_json, tokio.
- `src/main.rs` - fetch, normalize, overrides, validation, image check, atomic write, tests.
- `src/models.rs` - Valve payload structs and output structs.
- `README.md`, `.gitignore`, `overrides.json` (`{}`), `heroes/.gitkeep`, `heroes.json`.

No backend, database, API, Docker, CI, or extra modules. `main.rs` + `models.rs` only.

## Live endpoint schema findings (inspected before coding)

Hero list `https://www.dota2.com/datafeed/herolist?language=english`:

- Root path is `result.data.heroes[]`.
- Each entry: `id`, `name` (internal, e.g. `npc_dota_hero_antimage`), `name_loc`,
  `name_english_loc`, `primary_attr`, `complexity`.
- HTTP 200, ~16 KB. Returned **127** heroes, max id **155**.

Hero details `https://www.dota2.com/datafeed/herodata?language=english&hero_id={id}`:

- Root path is `result.data.heroes[]` with a single entry.
- Adds `attack_capability`, `role_levels` (9 entries), `abilities[]`, `talents[]`,
  `facet_abilities[]`.
- **Important:** `herodata` does **not** include `name_english_loc` (only `name_loc`);
  the English name therefore comes from the `herolist` entry. `name` is identical in
  both endpoints and is cross-checked in code.
- Ability entries carry `id`, `name` (internal), `name_loc`, `is_item`, `type`,
  `max_level`, and many unused fields (descriptions, damages, cooldowns...).

## Behavior

- Attribute map: `0->strength, 1->agility, 2->intelligence, 3->universal`; unknown errors.
- Attack map: `1->melee, 2->ranged`; unknown errors (never inferred from range).
- Roles: from `role_levels`, index order carry/support/nuker/disabler/jungler/durable/
  escape/pusher/initiator, included when level > 0.
- Abilities: only normal abilities, excludes `is_item`, `special_bonus_*`, and entries
  with a blank localized name; stores `{id, name(name_loc), internalName}`.
- Overrides: merged from `overrides.json`; `powerSpike` validated against
  null/early/mid/late; `hintAbilityId` validated to belong to that hero;
  `buildHint` is `Option<Vec<u32>>`. `overrides.json` is never written by the tool.
- Networking: 8 concurrent requests via semaphore, 20s timeout, 3 attempts with backoff
  on 429/500/502/503/504. Any unrecovered hero fetch aborts before writing.
- Safe output: full sync in memory, pretty JSON, temp file + a single `std::fs::rename`
  (which replaces an existing destination on Windows and Unix); the destination is never
  removed by the tool.
- Images: exact `dir.join("{id}.png").is_file()` check per hero; directories and
  non-exact names never count. Orphan reporting is limited to numeric `.png` file names
  that do not match a current hero id.
- Overrides: keys must be the canonical hero id string (e.g. `"45"`, not `"045"`);
  non-canonical keys abort the run instead of being silently ignored.

## Verification commands

| Command | Exit | Result |
| --- | --- | --- |
| `cargo fmt` | 0 | no diffs |
| `cargo test` | 0 | 19 passed, 0 failed |
| `cargo clippy --all-targets -- -D warnings` | 0 | no warnings |
| `cargo run --release` | 0 | 127 generated |
| `cargo run --release` (rerun) | 0 | overwrote existing `heroes.json` cleanly |
| invalid `hintAbilityId` run | 1 | aborted; `heroes.json` hash unchanged (preserved) |

Tests are offline unit tests: attribute mapping, invalid attribute, attack mapping,
invalid attack type, role normalization, talent/item/blank filtering, name preference,
normalization, override merge, override hint validation, unknown override hero,
non-canonical override key rejection, override parsing, camelCase serialization,
atomic overwrite, destination preservation on failed rename, exact image path checks
with numeric-only orphans, and required `role_levels`/`abilities` schema fields.

## Acceptance corrections applied

1. `write_atomic` now performs one `std::fs::rename` with a contextual error. The old
   fallback removed the destination on any rename error; that path is gone. Covered by
   `write_atomic_overwrites_existing_file` and `write_atomic_preserves_destination_on_failure`.
2. `check_images` checks the exact `heroes/{id}.png` file per hero, skips directories
   and non-exact names, and reports orphan files only when the numeric file name has no
   matching current hero id. Covered by
   `check_images_uses_exact_paths_and_numeric_orphans_only`.
3. Non-canonical override keys (for example `"045"`) are now rejected during validation.
   Covered by `override_validation_rejects_noncanonical_key`.
4. `role_levels` and `abilities` are required in the herodata payload; a missing schema
   field now fails deserialization instead of silently producing incomplete hero data.
   Covered by `hero_data_requires_role_levels_and_abilities`.

## Generated data

- Valve returned: **127** heroes.
- Generated: **127** heroes (`heroes.json`, sorted by id ascending, 133,803 bytes).
- Failed fetches: **0**.
- 65 ranged / 62 melee; 22 universal heroes
  (3,16,21,33,38,40,43,50,53,65,78,88,91,92,97,102,105,113,120,126,128,136).
- 734 abilities total.

### Missing hero images (all 127; `heroes/` is empty at baseline)

IDs: 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,25,26,27,28,29,30,
31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,
59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,
87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,
111,112,113,114,119,120,121,123,126,128,129,131,135,136,137,138,145,155

Missing images are warnings only and do not fail generation.

### Orphan images

**None.** Verified by adding temporary `heroes/9999.png` and `heroes/notnumeric.png`;
only 9999.png was reported as an orphan after the review correction; both test files were removed. The tool never deletes images.

## Requested sample heroes

| Hero | id | attribute | attack | complexity | roles | abilities |
| --- | --- | --- | --- | --- | --- | --- |
| Anti-Mage | 1 | agility | melee | 1 | carry, nuker, escape | 5 |
| Enigma | 33 | universal | ranged | 2 | disabler, pusher, initiator | 5 |
| Pugna | 45 | intelligence | ranged | 2 | nuker, pusher | 5 |
| Invoker | 74 | intelligence | ranged | 3 | carry, nuker, disabler, escape, pusher | 14 |
| Ringmaster | 131 | intelligence | ranged | 2 | support, nuker, disabler, escape | 10 |
| Kez | 145 | agility | melee | 3 | carry, disabler, escape | 9 |
| Largo | 155 | strength | melee | 2 | support, disabler, durable | 8 |

Custom fields (`powerSpike`, `signature`, `hintAbilityId`, `buildHint`) are all `null`
in the final `heroes.json`, because `overrides.json` is `{}`.

## Limitations / notes

- `heroes/` has no PNGs yet, so every hero is currently reported missing. This resolves
  once images are added as `heroes/{id}.png`; no code change is needed.
- Hero English name depends on the `herolist` payload (`herodata` omits it). If Valve
  ever drops `name_english_loc`, the tool falls back to `name_loc`.
- Live override merge was verified with `{"45":{"powerSpike":"mid","signature":"channeling",
  "hintAbilityId":5186}}` (correct output), then `overrides.json` was restored to `{}`.
- A standalone Git repository was initialized here after acceptance; nothing was committed.

