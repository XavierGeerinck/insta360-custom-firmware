# Kit fonts

Generated LVGL fonts for `src/kit`. Run `./build_fonts.sh` to regenerate. It downloads
pinned sources, so the output is reproducible. The generated `.c` files are committed.

| Font | Source | Sizes |
|---|---|---|
| `font_micro_12`, `font_small_13`, `font_body_15` | Barlow Semi Condensed Medium | 12, 13, 15 |
| `font_title_20`, `font_dial_26` | Barlow Semi Condensed SemiBold | 20, 26 |
| `font_num_13`, `font_num_20` | Barlow Medium | 13, 20 (full text range) |
| `font_num_26`, `font_num_48` | Barlow Medium | 26, 48 (digits, `% + - . :` and minus only) |
| `font_icons_16/20/26/44` | Tabler Icons webfont 3.48.0 | the 48 icons in `icons.py` |

- **Tabular figures:** every Barlow face has its `tnum` feature frozen with
  `pyftfeatfreeze`, so digits have equal widths and timers don't jitter.
- **Text range:** covers ASCII, Latin-1, curly quotes, en/em dashes, bullet, `…` and `−`.
- **Licences:** Barlow is OFL (`LICENSE-Barlow.txt`); Tabler Icons is MIT (`LICENSE-Tabler.txt`).
- **Adding an icon:** add it to `ICONS` in `icons.py` and rerun the script.
