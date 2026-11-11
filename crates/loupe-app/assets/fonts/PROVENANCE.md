# Embedded fonts

- Inter 4.1 (Regular, Medium, SemiBold, Bold, Italic), from the `extras/ttf` folder of https://github.com/rsms/inter/releases/tag/v4.1. SIL Open Font License 1.1: `Inter-LICENSE.txt`.
- JetBrains Mono 2.304 (Regular, Bold, Italic), from `fonts/ttf` of https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304. SIL Open Font License 1.1: `JetBrainsMono-LICENSE.txt`.

The files are unmodified. `assets::fonts` embeds them and `theme::init` registers them, so the interface does not depend on the fonts a machine has installed.
