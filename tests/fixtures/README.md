# Calibration adoption reference

`calibration-settings-before-seiza.json` was captured by calling
`psf_guard::calibration::rules::fields()` on the original PSF Guard library at
`79f5e31d53b8371e8fcd0b22a9b69692412cd8b4`, before adopting the Seiza policy.
Its `FIELDS` source block is identical at this PR's base, `835961cb`.

The settings API test compares all 43 controls, including labels, help, defaults,
ranges, units and choices. Do not regenerate this reference from Seiza's schema:
that would stop testing compatibility with the original host. An intentional
settings change needs a separate compatibility review and an explicit fixture
update.
