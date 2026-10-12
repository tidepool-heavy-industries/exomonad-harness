# IBM Plex Mono

Self-hosted regular (400) and semibold (600), unmodified WOFF2 assets from
https://github.com/IBM/plex/tree/763c36ef9117782905ae010056dfbe8fd2653a25/packages/plex-mono/fonts/complete/woff2

The included OFL.txt is the upstream SIL Open Font License. These implement the
existing code typeface choice without depending on an operator's installed fonts
or a third-party font service. Vite hashes the fonts into the browser bundle;
Buck includes this directory through WEB_PRODUCTION_SRCS.
ba204497f16b6d334cee9d1e963a831b73e3a56e1d6300a8489d18df7214b350  web/src/fonts/IBMPlexMono-Regular.woff2
6a825b4824c01cbb401e829e5a066a1818411bcb3538b5a5792c5ca9b82343c3  web/src/fonts/IBMPlexMono-SemiBold.woff2
