# Protomaps sprites (v4)

The light and dark sprites, 1x and 2x, of the local Protomaps profile, kept here because no npm package carries them;
`../../refresh.mjs` (`protomapsStyles`) copies them to `libs/protomaps/sprites/v4/`.

- Upstream: https://github.com/protomaps/basemaps-assets/tree/028c18f713baecad011301ff7a69acc39bcc2ae7/sprites/v4
  (commit `028c18f713baecad011301ff7a69acc39bcc2ae7`, October 31, 2025), unchanged.
- License: MIT. The basemaps-assets README says the sprites are derived from
  [tangrams/icons](https://github.com/tangrams/icons/blob/92510779634f4a006c61ea70e50cb8c52c765a81/LICENSE.md)
  (Copyright (c) 2017 Mapzen); the full text is in `libs/NOTICES.md` under `protomaps`.

To update: fetch the same eight files at a newer commit, then update the commit in this file and in `refresh.mjs`
(`PROTOMAPS_SPRITES_LICENSE`), and run `./moq update-js @protomaps/basemaps <version>` or `./moq verify-js`.
