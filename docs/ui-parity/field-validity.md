# Java field validity colors

Match Java `webroot-theme.css`: invalid inputs and textareas use #FEE immediately, and focused valid inputs and textareas use #EEE. Keep browser constraint validation and explicit field-error behavior.

The existing Chromium CSS layout check passes. UI-only proof is retained in the companion regression checkout at `.runs/ui-narrated-20261024/validity-style-fix/`: actual paired browser recordings, per-control computed appearance and Piper narration. Step 00004 shows both empty customer names with identical Java/Rust background rgb(255,238,238), foreground rgb(85,85,85), border rgb(204,204,204), 14px/20px Helvetica font and inset shadow.

This small runtime CSS change does not establish whole-screen or complete business-workflow parity. Frozen asset and executable hashes are in runtime.json; the executable is an existing build, not rebuilt source.
