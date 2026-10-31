# Main window surface brief

Q-Music, the desktop window. One surface: library browsing, playback and settings live
in the same frame, because a music player is something you keep open while you do
something else.

## Direction contract

**THESIS.** The window is a measuring instrument for a collection you already own, not a
storefront feed. It refuses the sidebar-plus-cover-grid-plus-centred-now-playing
arrangement every player ships, and refuses the hero-metric strip that a "now playing"
panel always turns into.

**OWN-WORLD.** Matte black ground `#0A0B0C`; bone ink `#EDEAE3`; one vermilion signal
`#E23A1F` spent only on the current row, the level plot and the active nav mark — never as
decoration. Hairlines at 9% bone separate everything; no cards, no radius above 4px, no
shadows. Archivo carries language; IBM Plex Mono carries every measurement, tabular and
right-aligned. Strip the words and it is still recognisable: full-bleed hairline rows, a
vermilion block on the current row, one wide level plot, numerals stacked down the right
edge.

**STORY.** Within two seconds the visitor knows the library is real and indexed — counts,
total listening time, format mix — knows what is playing, and knows how loud it is. Then
they press play on a row.

**FIRST VIEWPORT.** At 1440: a 208px rail of nav text over hairline separators, scan
status pinned to its foot; a full-bleed track table with search and sort in one hairline
header row; a 340px now-playing column holding square cover, album/artist/title and the
vermilion level plot at full column width. Playback sits in an 88px bar spanning the full
width at the bottom: playing identity at its left edge, transport and scrubber at its
centre, volume, speed and equaliser at its right. The primary action is a row's play
target, not a floating button. Signature interaction: the vermilion block travels between
rows on selection, one authored motion, and the level plot draws the engine's measured RMS
— silence draws silence.

**FIT.** The window's minimum is 960 × 620, and the interface is designed for it. Below
1180px the now column becomes an overlay drawer over the table, opened from the header,
with the transport still full width beneath it; the rail narrows to 190px and the volume
slider gives way to the transport tools. There is no phone layout, because a desktop
window cannot be dragged smaller than its minimum.

**FORM.** Catalogue sleeve — matte ground, hairline plot, colour-block technical strip,
numbers read as a code. Position 1 of 8 on the ordered list, after the seed roll assigned
index 7 and this challenger won both the direction and craft axes.
seed:f450aa7df4515155

**FINISH.** unreviewed and undocumented is unfinished; this build ends with the finish
review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
