# Review: PercyStar promo v1

Render: `renders/percystar-promo.mp4`, 1920x1080, 30 fps, 31.3 s.

## Creative calls
- Built storyline A, "Out of the archive". B ("Bytes to things") and C ("One window rebuilds") are in STORYLINES.md.
- Palette taken from the app's own `PercyStarPalette.xml`: #120C1C, #FFA81C and #FF48A5.
- The window chrome is rebuilt with real labels: list rows, category counts, mode buttons and the `sunset_coupe.#ft · 168 verts · 296 tris` info line.
- The status bar folder path is shortened to `percystar_demo_folder`, so it shows no Windows user path.
- The 3D car shots are the real 12 fps viewport recording, upscaled 2x.
- The hex is the first 960 bytes of a real RPF9 header (`9FPR`).
- The exported file name `sunset_coupe.ft.obj` follows the app's own Save rule (`mesh.name` with `#` removed).

## Measured
- `check`: ok.
- Rendered audio: -15.7 LUFS, true peak -6.5 dB, no clipping. The onsets fall 1 frame after each click.
- The sound was measured, not heard.

## Kept on purpose
- `text_below_floor`: the full-window establishing shots show the real UI at native size. They are context; every action is punched in to 2.6-3.2x.
- `contrast_aa_failure`: white text on the #FF48A5 pill is the app's real selected-pill style.
- `nested_media_start_basis_ambiguous`: the viewport video starts are scene-local, as intended.

## Weakest shot
The car footage is 12 fps and soft. A Retina capture of the app at 60 fps would fix it, but needs `percystar_showcase.rpf` on the Mac.

## Sound
- No music, by request. Sound effects only.
