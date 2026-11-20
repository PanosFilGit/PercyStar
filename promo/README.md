# PercyStar promo

The 31 s promo video for PercyStar is a [HyperFrames](https://hyperframes.heygen.com) project. The rendered file is [`../assets/promo.mp4`](../assets/promo.mp4).

- `SPEC.md` has the shot list. `STORYLINES.md` has the three options, of which "Out of the archive" was built. `REVIEW.md` has the QA notes.
- `compositions/` holds one scene per file: hook, bytes, proof, payoff, endcard.
- The car shots in `assets/` are cut from the real PercyStar demo recording.

## Sound effects

The Pixabay sound effects are not committed, because their licence does not allow standalone redistribution. Before rendering, copy these files from the HyperFrames `media-use` skill bundle (`audio/assets/sfx/`) into `assets/sfx/`:

- `click.mp3`
- `click-soft.mp3`
- `typing.mp3`
- `whoosh.mp3`
- `whoosh-short.mp3`
- `pop.mp3`
- `riser.mp3`
- `impact-bass-1.mp3`

## Render

```bash
npx hyperframes check
npx hyperframes render -o renders/percystar-promo.mp4
```
