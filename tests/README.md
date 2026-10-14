# Screenshot regression tests

The screenshot tests render the real Slint software backend at the phone's
1206 × 2622 scanout size and 3× scale. Time advances explicitly in 16ms steps;
input coordinates and content are fixed. No desktop, display server, or QEMU
is needed for these tests.

The rising captures use the real reactive animation with a simulated clock,
not a wall-clock delay. The keyboard binding is initialized before the reveal
request. At the mid-slide timestamp, tests repeat reactive updates and redraws
without advancing time and assert pixel equality, guarding against evaluation
order causing motion or displacement drift.

Run the existing baselines:

```sh
cargo test -p alloyos-keyboard -p alloyos-ferrofluid --features alloyos-keyboard/preview --test screenshots
```

System UI covers boot, the workspace, a rising/open keyboard, a pressed-key
preview, dismissal, a semantic Clock app tile with its Refresh action, the animated
thinking indicator, and the bottom of a streamed reply. The clock descriptor comes
from the actual built-in MCP Clock app with a fixed uptime tick. Streaming appends 36 chunks and
asserts that the viewport follows the bottom after each update. Indicator redraws
at a fixed timestamp must remain identical and advancing time must animate the
dots. The independent keyboard scene covers rise, rest,
press, release, symbols, and scroll dismissal. Baselines live in each crate's
`tests/screenshots/` folder as RGB PNGs. Tests compare every pixel exactly.
All cases in a suite are checked before the suite reports its failures.

The clock follow-up captures open the keyboard after an embedded Clock card,
type and send with the soft keys, and check that the new message and loading
indicator remain above the composer. The assertion uses the rendered end of
the conversation, so an underestimated scroll range cannot pass merely by
reporting that its offset equals its maximum.

On failure, inspect `target/screenshots/<crate>/`: `*.actual.png`,
`*.expected.png`, and `*.diff.png`. Changed pixels are highlighted in pink;
the failure message includes the changed pixel count and artifact path.
Missing baselines and changed dimensions also fail.

For an intentional visual change, explicitly generate new baselines:

```sh
ALLOYOS_UPDATE_SCREENSHOTS=1 cargo test -p alloyos-keyboard -p alloyos-ferrofluid --features alloyos-keyboard/preview --test screenshots
```

Review the changed PNGs before committing them, then run the first command
without the update variable to verify them. Do not regenerate baselines merely
to silence an unexpected failure.

`xtask ci` and the pre-push hook include these tests and remove
`ALLOYOS_UPDATE_SCREENSHOTS` from the test environment, so uploads cannot silently
accept new baselines. The PNG dependency and screenshot helpers are host-only;
they are excluded from the bare-metal runtime.
