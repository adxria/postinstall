# Testing judgment

Every automated test should have a clear purpose: protecting meaningful behavior or a consequential failure. Choose coverage based on risk, not test counts or implementation details.

Keep tests focused and readable. Decide whether to add, extend, reorganize, or omit a test based on clarity and purpose; avoid both redundant tests and oversized scenarios.

Do not add tests for cosmetic changes, trivial implementation details, or redundant permutations. Use visual inspection for styling.

Run checks appropriate to the change, and expand validation only when there is a concrete unresolved concern.
