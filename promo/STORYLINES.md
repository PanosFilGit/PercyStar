# PercyStar promo: storylines

Shared assumptions: 25-35 s, 1920x1080 at 30 fps, music and UI sound only (no voiceover). Free local music and sound effects, nothing paid. Every screen is real PercyStar UI.

## A. Out of the archive (recommended)

The result comes first. The film is one continuous camera move with endless zooms. It follows the viewer's one task: get a car out of a sealed `.rpf` and into a usable `.OBJ`.

- **First 2 s:** the pink `sunset_coupe` turns in the 3D viewer, full frame, already working. The camera pulls back. The car is one row among hundreds in the archive list, and the status bar reads `1 RPFs`.
- **Beats:** hook (the car spins) > tension (a wall of raw `9FPR` bytes, unreadable) > reveal ("PercyStar" titled over the archive tree) > proof:
  1. Open Folder
  2. Search "sunset"
  3. Click the row
  4. Clay > Wire > Textured
  5. Export .OBJ

  Then payoff (the exported `.obj` drops into a file, still spinning) > claim > end card (logo carried in, "Download for Windows and macOS").
- **Look:** the app's own Neon Sunset palette: plum-black ground, `#FCAF17` orange, hot magenta. Real UI pushed to 1.6-2x, and nothing floats on a void.
- **Sound:** a slow synthwave bed. Each click, view switch and the export land as soft UI ticks, with a bass hit on the payoff.

## B. Bytes to things

Before and after, carried by type and cut on the beat.

- **First 2 s:** a full-screen hex dump scrolls fast, the `9FPR` magic highlighted. The line "4 GB of this" slams in.
- **Beats:** hook (hex) > tension (scrolling never ends) > reveal (the hex collapses into PercyStar's archive tree) > proof:
  1. Vehicles
  2. 3D Models
  3. Audio (ADAT Opus decodes and plays)
  4. Configs (XML)

  Then payoff (all categories light up with counts) > claim ("No FFmpeg. No Python. Just Rust") > end card.
- **Look:** monospace hex in dim plum, giving way to the app's UI colours, with hard cuts every half beat.
- **Sound:** a driving 120 bpm electronic track, with a cut on every beat.

## C. One window rebuilds (bold)

One frame rebuilds: the PercyStar window never leaves the screen and keeps rearranging itself.

- **First 2 s:** the empty PercyStar window. The status reads "Open an .rpf or select a game folder", then a folder drops in and rows pour down the list.
- **Beats:** hook (empty window) > tension (rows flood past) > reveal (the search box narrows to one car) > proof:
  1. The list pane shrinks and the viewer grows to fill the window
  2. Wire, then Clay, then Textured
  3. An audio stream plays as a waveform

  Then payoff (the window splits into a Windows and a macOS copy) > claim > end card.
- **Look:** the real window at 2x, with panes sliding and resizing as the transitions. No cuts.
- **Sound:** an ambient pad, with every pane move landing on a soft click.

## Source note

The only real UI on hand is the 960x582, 12 fps demo recording and the 1080x655 `preview.png`. Pushed to 1080p, these come out soft. A crisp result needs the `percystar_showcase.rpf` on this Mac, so the app can be captured at Retina resolution and 60 fps.
