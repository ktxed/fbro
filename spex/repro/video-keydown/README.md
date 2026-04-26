# video-keydown diagnostic

Goal: figure out *why* `<video controls>` blocks JS keydown on macOS, by
running the same scenarios on every engine on every OS and reading the
truth out of the data.

## Scenarios

Each scenario installs **bubble** and **capture** keydown counters on
`window`, then clicks a target element and presses `j k j k`. Expected
counter value when keys reach JS = **4** in each column.

| ID | What's tested |
|----|---------------|
| A  | sanity: a button, no video |
| B  | the bug case: `<video controls>` |
| C  | `<video autoplay muted>` (no controls) |
| D  | `<video controls tabindex="-1">` |
| E  | `<video controls onfocus="this.blur()">` |
| F  | D + E combined |
| G  | controls + every WebKit feature flag turned off |
| H  | controls + transparent overlay div with `tabindex=0` |

## Run

```bash
cd spex/repro/video-keydown
npm run setup           # one-time: install Playwright + browsers
npm run run             # all 3 engines
npm run run:webkit      # only webkit (the one that matters for macOS)
npm run run:headed      # watch it happen
```

Pass `--video=/abs/path/to/sample.mp4` to use a local file instead of
the default public URL — useful if the network is restricted.

## Reading results

For each engine, you'll get a row per scenario:

```
✅ A_baseline_no_video                           4        4        BODY         sanity
❌ B_video_controls                              0        0        VIDEO        the bug case
✅ D_video_controls_tabindex_minus_one           4        4        BODY         controls + tabindex=-1
...
```

- `bubble` / `capture` = how many keydowns reached the window listener.
- `activeTag` = `document.activeElement` after the click.
- ❌ = 0 keys received (full block), ⚠️ = partial, ✅ = 4 received (clean).

Send me the tables from Linux + macOS and we'll know exactly which
workaround actually defeats AVKit, instead of guessing.
