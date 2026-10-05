# Kit fonts

Generated LVGL fonts for `src/kit`. Run `./build_fonts.sh` to regenerate. It downloads
pinned sources, so the output is reproducible. The generated `.c` files are committed.

| Font | Source | Sizes |
|---|---|---|
| `font_micro_12`, `font_small_13`, `font_body_15` | Barlow Semi Condensed Medium | 12, 13, 15 |
| `font_title_20`, `font_dial_26` | Barlow Semi Condensed SemiBold | 20, 26 |
| `font_num_13`, `font_num_20` | Barlow Medium | 13, 20 (full text range) |
| `font_num_26`, `font_num_48` | Barlow Medium | 26, 48 (digits, `% + - . :`, minus, and `a` `m` `p` for am/pm) |
| `font_icons_16/20/22/26/44` | Tabler Icons webfont 3.48.0, outline face plus the filled face for names ending in `-filled` | the 50 icons in `icons.py` |

- **Tabular figures:** every Barlow face has its `tnum` feature frozen with
  `pyftfeatfreeze`, so digits have equal widths and timers don't jitter.
- **Text range:** covers ASCII, Latin-1, Latin Extended-A (U+0100-U+017F, so Polish, Czech and
  Hungarian network names render), curly quotes, en/em dashes, bullet, `…` and `−`.
- **Reproducible:** `lv_font_conv` runs inside the temp dir with relative paths, so the `Opts:` header
  never embeds a machine-specific path; rerunning the script leaves every file byte-identical.
- **Licences:** Barlow is OFL (`LICENSE-Barlow.txt`); Tabler Icons is MIT (`LICENSE-Tabler.txt`).
- **Adding an icon:** add it to `ICONS` in `icons.py` and rerun the script.
- **Filled icons:** a name ending in `-filled` (`player-stop-filled`) comes from Tabler's
  filled face, which has its own code points; the media controls (play, pause, stop, record)
  use it so Stop is a solid square and Record a solid dot, as in the mockups.
