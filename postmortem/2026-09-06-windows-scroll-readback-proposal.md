# Windows scroll readback proposal

## What happened

The experiments in upstream issue #273 reported blank or stale rows after scrolling. The original mixed commit `4bc1c6b` also changed session restore defaults, making the rendering change difficult to evaluate separately.

## Root cause

The experimental diagnosis was that a viewport transition could reuse dirty-row readback regions from the prior viewport. This has not been reproduced on beta.95: upstream now has stronger snapshot recovery and image damage tracking.

## Fix proposed

Force a draw and full readback when the viewport offset changes. Retain partial readback for an unchanged viewport. Record the offset only after drawing and copy submission succeed so an error cannot suppress a retry. Keep upstream's cursor viewport visibility; the old `offset == 0` cursor condition is not carried forward.

## Validation still required

On Windows, compare the parent and this branch with the same machine, font, DPI, window size and scrollback. Use the existing `CON_GHOSTTY_PROFILE=1` and `CON_GHOSTTY_PROFILE_VERBOSE=1` instrumentation. Exercise wheel scrolling, scrollbar dragging, resize, sustained TUI output and Kitty images. Report median/p95 frame time, readback rows/bytes, CPU/GPU usage and any stale-frame reproduction. Keep this proposal in draft until the measurements establish whether it is still necessary and acceptable.

## What we learned

Viewport invalidation and transcript persistence need separate review. Older experimental fixes must be reassessed against current upstream recovery guarantees.
