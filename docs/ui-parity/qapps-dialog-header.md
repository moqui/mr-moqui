# qapps dialog header

Use Java's card-actions header, h5 title and title spacing in m-dialog. This restores the reference title weight/size and close-button placement within the header. Keep Vue 3 events and existing drag handling.

Validation: 22 Vue component checks and two CSS browser checks pass. Actual UI run: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-dialog-header-styled`, eight paired steps with Piper narration. Both create UI / Proof6 and reload both saved names. Both shutdown checks pass (`.runs/2026-10-25_07-40-42.074312Z/shutdown.json`).

Remaining: Country and State dropdowns are four pixels wider than Java; State wraps, making the dialog 52 pixels taller. Close and most fields consequently remain 26 pixels above Java, with Create 26 below. This PR does not establish screen parity. The earlier header-only capture without the title CSS remains separately retained. Existing binary with frozen runtime assets, not a fresh Rust build.
