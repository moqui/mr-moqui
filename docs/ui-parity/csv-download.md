# HTML report CSV download

The HTML client now exports `moqui.downloadData(data, filename, type)`, which existing chart templates call from Get CSV buttons. It creates a Blob with the supplied MIME type and payload, clicks an attached download link using the supplied filename, then removes the temporary link and revokes the object URL. Previously the click raised `moqui.downloadData is not a function`.

Validation: 8 client checks and the Chromium download regression pass. The browser regression reproduces the original missing-method failure before the patch, then verifies filename, Unicode CSV payload, MIME type, attached link and cleanup after the patch.

Actual paired UI evidence: java-regression-test `.runs/ui-narrated-20261024/report-csv-download`, step 4. Both browsers receive an actual download event and save `OperatingRevenueExpense.csv`; both files are empty and share SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`. Four narrated screencast clips are complete. Both engines shut down without forced kill or missing shutdown markers (`.runs/2026-10-25_06-29-37.366658Z/shutdown.json`).

Limits: reference chart data was empty, so this proves the download button/file mechanism, not populated accounting report accuracy. Shared-coordinate clicks reached the buttons, but their measured bounds and resulting full screens still differ. Runtime proof uses the existing Rust executable with frozen changed runtime assets, not a fresh Rust source build.
