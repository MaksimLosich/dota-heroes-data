# heroes_data

Small Rust generator for DotaWho hero data.

## Source

Valve official Dota 2 datafeed:

- Hero list: `https://www.dota2.com/datafeed/herolist?language=english`
- Hero details: `https://www.dota2.com/datafeed/herodata?language=english&hero_id={HERO_ID}`

No Steam API key is required. No third-party data sources are used.

## Run

```
cargo run --release
```

## Test

```
cargo test
```

## Output

`heroes.json` - JSON array sorted by hero id ascending, camelCase field names.

Valve's hero id is the canonical identifier used everywhere.

## Images

`heroes/{hero_id}.png`

Images are checked, never modified. Missing images are reported as warnings and do
not fail generation. Numeric PNG files with no matching current hero are reported as
orphans and are never deleted.

## Manual fields

`overrides.json` supplies DotaWho-specific fields that Valve does not provide:
`powerSpike`, `signature`, `hintAbilityId`, `buildHint`. It defaults to `{}` and is
merged into the generated data on every run. It is never overwritten, and
`hintAbilityId` must reference an ability that belongs to that hero.

```json
{
  "45": {
    "powerSpike": "mid",
    "signature": "channeling",
    "hintAbilityId": null,
    "buildHint": null
  }
}
```

`heroes.json` is only written after a complete successful sync, using a temporary
file and an atomic replacement.

## Related

The Flutter app that consumes this data (DotaWho) is maintained in a separate
repository.
