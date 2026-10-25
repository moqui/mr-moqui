# Initial dependent qapps options

A dynamic dropdown with one dependency now loads its initial options. The previous child-count condition omitted initial loading for state/province. With the country already set, Java displayed Alabama and allowed Create; Rust left the required selector empty and blocked the same form with “Please select an option.” Dependency changes continue to use the existing refresh behavior.

Validation: 15 QVT macro and 22 Vue client checks pass (37 total). The new regression fails before the change for a state dropdown with one country dependency.

UI proof: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-dropdown-init`, eight paired screen steps with Piper narration. Both show AL - Alabama on opening, accept UI / Proof6 in the name fields, submit Create, reach Customer/EditCustomer and retain both names after a browser reload. The earlier `qapps-customer-persistence` run retains the failed Rust submission. Source commit/diff, frozen assets and artifact hashes are recorded in runtime.json.

Limits: qapps control coordinates and full-screen parity still differ; the recorder discloses recovery clicks. This establishes only customer creation and saved-name reload, not all customer activities. Runtime proof uses the existing binary with changed macros; no fresh Rust source build is claimed. Both engines shut down with no forced kill or missing shutdown markers in `.runs/2026-10-25_07-22-14.591567Z/shutdown.json`.
