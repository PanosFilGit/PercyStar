# SPEC: PercyStar, "Out of the archive"

GOAL:    the viewer sees a car pulled out of a sealed .rpf into a .obj file, and wants to download PercyStar.
OUTPUT:  HyperFrames, 1920 x 1080, 30 fps
TIME:    31 s x 30 = 930 frames. No music yet (HeyGen sign-in needed); bundled UI SFX only.
LOOK:    The app's own palette, taken from its PercyStarPalette.xml: backdrop #120C1C, highlight #FFA81C, accent #FF48A5.
         UI panels #18101F / #120C1C, widgets #221632 with a 1 px #442A62 stroke. Inter 400/700/900.
         Titles: Inter 900 at 104 px, -2% tracking, white #F4EEF8. The accent goes on one word per title.
         Medium: real product UI. preview.png and the real 12 fps viewport recordings; the window chrome is rebuilt at native 1080x655 coordinates with the real labels.
         Hex: JetBrains Mono 30 px, #4A3866, with the 9FPR magic in #FFA81C.
OBJECTS: the PercyStar window (topbar, archive tree, list, 3D viewport), the pink sunset_coupe, a cursor arrow 26 native px, the .obj file chip.
MOTION:  enter expo.out 0.5-0.7 s; camera power2.inOut 0.5 s; zoom-through power3.in 0.45 s; exits 0.25 s.
         Titles per word blur(8px) -> 0 in 0.14 s, 0.08 s apart, then dead still.
SHOTS:
  0.0-3.6   hook: real textured coupe spins at full height (viewport x2.1). Pull back to the whole real window (1.6-2.4).
            Track to the "Hex / Info" tab and zoom through it (2.8-3.6). whoosh.
  3.6-9.0   bytes: real 9FPR header hex scrolls up (linear). "Inside, it's just bytes" lands at 4.2 and holds.
            6.4: the hex rows collapse, and "PercyStar" lands per letter, orange. 8.7: whip left into the window.
  9.0-21.0  proof, in the rebuilt window with a camera at 1.75-3.2x. Each click causes the next state:
            Open Folder -> rows pour in -> type "sunset" -> click the VEH row -> the viewport plays the real coupe ->
            Clay Solid -> Wire -> Textured/Mat -> Export .OBJ (the file chip leaves the button).
  21.0-25.4 payoff: the coupe fills the frame. The chip "sunset_coupe.ft.obj" lands with "168 verts · 296 tris". pop.
  25.4-31.0 claim on an orange flood: "Open the archive. Keep the model." Then the end card: logo carried in,
            wordmark wipe, "Download for Windows and macOS", github.com/rpatty-dev/PercyStar, held 2.6 s. riser -> bass impact.
BANNED:  dark void with floating UI, glow and particles, three cards, everything fading up, a docked logo,
         a crossfade between worlds, invented UI labels, user paths in the status bar.
QA:      t=0 (car fills the frame), 2.4 (whole window), 4.8 (hex and title), 7.6 (PercyStar),
         12.6 (search typed), 14.6 (car in the viewport), 16.8 (wire), 22.5 (chip), 26.6 (claim), 30.0 (end card).
