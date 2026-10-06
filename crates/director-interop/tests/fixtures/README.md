# AstroCollab fixtures

`starfront-*.json` are synthetic public examples captured from Starfront by
AstroCollab, copied byte-for-byte from `examples/` at
`theatrus/astrocollab-api@35a6f068c6015367b4dbb69c1c4de5b7a0747d90`.
They contain no credentials or private telescope data.

`reference-tonight.json` is a response from that revision's independent
reference server, with the sample M31 project, the example hello profile,
night `2026-10-05`, fixed test clock `1791171000`, and a dark-night join.
The test telescope was enrolled only in a fresh in-memory loopback server.
Only the nightly response was retained; tokens and pairing codes were not.

These fixtures exercise read-only normalization, not acquisition, HTTP
authentication or file reporting. AstroCollab's MIT license is reproduced in
`ASTROCOLLAB-LICENSE.txt`. No unlicensed Starfront implementation code is copied.
