# Documentation

The root [README](../README.md) gives the product tour and installation path.
Use this index for detailed behavior and engineering records.

## Catalog and review workflows

- [Adding FITS folders](IMPORTING.md)
- [Calibration libraries](CALIBRATION_LIBRARY.md)
- [Project stack previews](STACKING_PREVIEWS.md)
- [Statistical grading](STATISTICAL_GRADING.md)

## Quality and sky analysis

- [Quality screening](SCREENING.md)
- [Astrometry quality grading](ASTROMETRY_QUALITY.md)
- [Satellite track prediction](SATELLITES.md)

## Active design records

These files explain contracts that span several modules or still have planned
work. Historical checklists record delivery; they do not track current work.

- [Data transfer and remote sync](design/data-transfer.md)
- [Multi-database architecture](design/multi-database.md)
- [Reject archive safety model](design/reject-archive.md)

## Component-specific guides

- [App and server updates](UPDATES.md)
- [RPM packaging](../packaging/rpm/README.md)
- [Target Scheduler schema snapshot](../src/ts_schema/README.md)
- [Web frontend](../static/README.md)
- [Playwright FITS fixtures](../static/e2e/fixtures/README.md)

Contributor and agent rules live in [CLAUDE.md](../CLAUDE.md). `AGENTS.md`
links to the same file.
